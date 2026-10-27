//! R35's stream receiver for the 12c probe: the dress sink's RTMP dialogue
//! (`dress-rehearsal.test.ts`, `startRtmpSink`), with every video message
//! reassembled whole and written to an FLV file with its timestamp, so the
//! stream as it arrived can be read back frame by frame. ffmpeg's own listener
//! refuses the engine's publisher; this answers the three commands it sends
//! (`connect`, `createStream`, `publish`) and nothing more.
//!
//! The engine's publisher sends fmt 0 first chunks (absolute timestamps) and
//! fmt 3 continuations only, so those are the only chunk formats framed here;
//! anything else is recorded as a parse error and the file stops there.

import { createWriteStream } from "node:fs";
import { createServer, type Socket } from "node:net";

export interface FlvReceiver {
  port: number;
  /** Video messages written to the file. */
  videoMessages: () => number;
  /** The first chunk the receiver could not frame, if any. */
  parseError: () => string | null;
  /** Close the connection and finish the file. */
  close: () => Promise<void>;
}

const MSG_SET_CHUNK_SIZE = 0x01;
const MSG_VIDEO = 0x09;
const MSG_COMMAND_AMF0 = 0x14;

function amfString(s: string): Buffer {
  const b = Buffer.alloc(3 + s.length);
  b[0] = 0x02;
  b.writeUInt16BE(s.length, 1);
  b.write(s, 3, "latin1");
  return b;
}
function amfNumber(n: number): Buffer {
  const b = Buffer.alloc(9);
  b[0] = 0x00;
  b.writeDoubleBE(n, 1);
  return b;
}
const AMF_NULL = Buffer.from([0x05]);
function amfObject(props: Array<[string, Buffer]>): Buffer {
  const parts: Buffer[] = [Buffer.from([0x03])];
  for (const [k, v] of props) {
    const key = Buffer.alloc(2 + k.length);
    key.writeUInt16BE(k.length, 0);
    key.write(k, 2, "latin1");
    parts.push(key, v);
  }
  parts.push(Buffer.from([0x00, 0x00, 0x09]));
  return Buffer.concat(parts);
}
/** One AMF0 command message on chunk stream 3, single chunk (payload ≤ 128). */
function rtmpCommand(streamId: number, payload: Buffer): Buffer {
  const h = Buffer.alloc(12);
  h[0] = 0x03;
  h.writeUIntBE(0, 1, 3);
  h.writeUIntBE(payload.length, 4, 3);
  h[7] = MSG_COMMAND_AMF0;
  h.writeUInt32LE(streamId, 8);
  return Buffer.concat([h, payload]);
}

/** Frames the client's chunks into whole messages, each with its timestamp. */
class Reassembler {
  private buf = Buffer.alloc(0);
  private chunkSize = 128;
  private cur: { type: number; ts: number; remaining: number; ext: boolean; body: Buffer[] } | null = null;
  parseError: string | null = null;
  constructor(private onMessage: (type: number, ts: number, body: Buffer) => void) {}
  feed(chunk: Buffer): void {
    this.buf = Buffer.concat([this.buf, chunk]);
    while (this.parseError === null && this.step()) {
      // keep framing
    }
  }
  /** Frame one whole chunk; false when more bytes are needed. */
  private step(): boolean {
    if (this.buf.length < 1) return false;
    const b0 = this.buf[0] as number;
    const fmt = b0 >> 6;
    if ((b0 & 0x3f) < 2) {
      this.parseError = `unexpected multi-byte chunk stream id ${b0 & 0x3f}`;
      return false;
    }
    let hdr: number;
    let msg: { type: number; ts: number; remaining: number; ext: boolean; body: Buffer[] };
    if (fmt === 0) {
      if (this.buf.length < 12) return false;
      const ext = this.buf.readUIntBE(1, 3) === 0xffffff;
      if (ext && this.buf.length < 16) return false;
      hdr = 12 + (ext ? 4 : 0);
      const ts = ext ? this.buf.readUInt32BE(12) : this.buf.readUIntBE(1, 3);
      msg = { type: this.buf[7] as number, ts, remaining: this.buf.readUIntBE(4, 3), ext, body: [] };
    } else if (fmt === 3 && this.cur !== null && this.cur.remaining > 0) {
      hdr = 1 + (this.cur.ext ? 4 : 0);
      msg = this.cur;
    } else {
      this.parseError = `unexpected fmt ${fmt}`;
      return false;
    }
    const take = Math.min(msg.remaining, this.chunkSize);
    if (this.buf.length < hdr + take) return false;
    msg.body.push(Buffer.from(this.buf.subarray(hdr, hdr + take)));
    this.buf = this.buf.subarray(hdr + take);
    msg.remaining -= take;
    this.cur = msg;
    if (msg.remaining === 0) {
      const body = Buffer.concat(msg.body);
      if (msg.type === MSG_SET_CHUNK_SIZE && body.length >= 4) this.chunkSize = body.readUInt32BE(0) & 0x7fffffff;
      else this.onMessage(msg.type, msg.ts, body);
    }
    return true;
  }
}

/** One FLV video tag: its 11-byte header, the message, the previous-tag size. */
function flvVideoTag(ts: number, body: Buffer): Buffer {
  const h = Buffer.alloc(11);
  h[0] = MSG_VIDEO;
  h.writeUIntBE(body.length, 1, 3);
  h.writeUIntBE(ts & 0xffffff, 4, 3);
  h[7] = (ts >>> 24) & 0xff;
  const tail = Buffer.alloc(4);
  tail.writeUInt32BE(11 + body.length, 0);
  return Buffer.concat([h, body, tail]);
}

/** Listen on 127.0.0.1 (a free port) and write the published video to `path`. */
export async function startFlvReceiver(path: string): Promise<FlvReceiver> {
  const file = createWriteStream(path);
  // The FLV header (video only) and PreviousTagSize0.
  file.write(Buffer.from([0x46, 0x4c, 0x56, 0x01, 0x01, 0x00, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00, 0x00]));
  let videoMessages = 0;
  let parseError: string | null = null;
  const sockets = new Set<Socket>();
  const srv = createServer((sock) => {
    sockets.add(sock);
    sock.on("close", () => sockets.delete(sock));
    sock.on("error", () => {});
    let phase: "c0c1" | "c2" | "chunks" = "c0c1";
    let buf = Buffer.alloc(0);
    const reassembler = new Reassembler((type, ts, body) => {
      if (type === MSG_VIDEO) {
        videoMessages += 1;
        file.write(flvVideoTag(ts, body));
        return;
      }
      if (type !== MSG_COMMAND_AMF0) return;
      if (body.subarray(0, 10).equals(amfString("connect"))) {
        sock.write(rtmpCommand(0, Buffer.concat([
          amfString("_result"), amfNumber(1), AMF_NULL,
          amfObject([["code", amfString("NetConnection.Connect.Success")]]),
        ])));
      } else if (body.subarray(0, 15).equals(amfString("createStream"))) {
        sock.write(rtmpCommand(0, Buffer.concat([amfString("_result"), amfNumber(2), AMF_NULL, amfNumber(1)])));
      } else if (body.subarray(0, 10).equals(amfString("publish"))) {
        sock.write(rtmpCommand(1, Buffer.concat([
          amfString("onStatus"), amfNumber(0), AMF_NULL,
          amfObject([["code", amfString("NetStream.Publish.Start")]]),
        ])));
      }
    });
    sock.on("data", (chunk: Buffer) => {
      if (phase === "chunks") {
        reassembler.feed(chunk);
        parseError = reassembler.parseError;
        return;
      }
      buf = Buffer.concat([buf, chunk]);
      if (phase === "c0c1" && buf.length >= 1537) {
        const c1 = buf.subarray(1, 1537);
        sock.write(Buffer.concat([Buffer.from([3]), Buffer.alloc(1536), c1]));
        buf = buf.subarray(1537);
        phase = "c2";
      }
      if (phase === "c2" && buf.length >= 1536) {
        phase = "chunks";
        reassembler.feed(buf.subarray(1536));
        parseError = reassembler.parseError;
        buf = Buffer.alloc(0);
      }
    });
  });
  await new Promise<void>((r) => srv.listen(0, "127.0.0.1", () => r()));
  const addr = srv.address();
  if (addr === null || typeof addr !== "object") throw new Error("the receiver has no address");
  return {
    port: addr.port,
    videoMessages: () => videoMessages,
    parseError: () => parseError,
    close: async () => {
      for (const s of sockets) s.destroy();
      srv.close();
      await new Promise<void>((r) => file.end(() => r()));
    },
  };
}
