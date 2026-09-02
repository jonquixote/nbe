/**
 * GENERATED FROM schemas/manifest.v0.3.json — do not edit (addendum 02a §1.4).
 * Regenerate: npm run gen:manifest-types.
 */

export type Id = string;
export type Pulldown =
  | {
      mode: "pattern";
      /**
       * @minItems 1
       */
      pattern: [number, ...number[]];
    }
  | {
      mode: "repeatNthSourceFrame";
      n: number;
    }
  | {
      mode: "repeatOnePerNSourceFrames";
      n: number;
    };
export type Item = {
  id: Id;
  kind: "sceneRef" | "sequenceRef" | "clipRef" | "liveRef" | "slate";
  sceneRef?: Id;
  sequenceRef?: Id;
  assetId?: Id;
  sourceId?: string;
  durationFrames?: number;
  autoFollow?: boolean;
  audioPolicy?: "clip" | "bed" | "mute";
} & {
  id: Id;
  kind: "sceneRef" | "sequenceRef" | "clipRef" | "liveRef" | "slate";
  sceneRef?: Id;
  sequenceRef?: Id;
  assetId?: Id;
  sourceId?: string;
  durationFrames?: number;
  autoFollow?: boolean;
  audioPolicy?: "clip" | "bed" | "mute";
};
export type Element = {
  id: Id;
  kind: "videoLoop" | "clip" | "camera" | "guest" | "graphic" | "ticker" | "clock" | "sceneRef" | "group" | "plugin";
  z: number;
  visible?: boolean;
  assetId?: Id;
  feedAssetId?: Id;
  cameraId?: string;
  guestId?: string;
  templateId?: Id;
  fields?: {
    [k: string]: unknown;
  };
  loop?: LoopMetadata;
  transform?: Transform;
  opacity?: number;
  chromaKey?: ChromaKey;
  audio?: LayerAudio;
  clock?: ClockConfig;
  sceneRef?: Id;
  pluginId?: Id;
  children?: Id[];
  enterAnimation?: Animation;
  exitAnimation?: Animation;
};

export interface Manifest {
  manifestVersion: "0.3";
  network: Network;
  channel?: Channel;
  show: Show;
  /**
   * @minItems 1
   */
  assets: [Asset, ...Asset[]];
  templates?: GraphicTemplate[];
  rundown: Sequence;
  control: Control;
  features?: Features;
  /**
   * @minItems 1
   */
  scenes: [Scene, ...Scene[]];
  overlays?: Overlay[];
  transitions?: TransitionPreset[];
  automation?: AutomationRule[];
  plugins?: Plugin[];
  qualityProfile?: "potato" | "consumer" | "pro" | "reference";
}
export interface Network {
  id: Id;
  name: string;
  logoAssetId?: Id;
  fallbackAudioAssetId?: Id;
}
export interface Channel {
  id: Id;
  name?: string;
  futureScheduler?: {
    [k: string]: unknown;
  };
  [k: string]: unknown;
}
export interface Show {
  id: Id;
  title: string;
  episodeCode?: string;
  video: VideoSpec;
  audio: AudioSpec;
  transitions?: TransitionDefaults;
  fallbackAssetId: Id;
  outputs?: OutputDefaults;
}
export interface VideoSpec {
  width: number;
  height: number;
  frameRate: 30 | 60;
  colorSpace: "rec709";
  aspectRatio?: "16:9";
}
export interface AudioSpec {
  sampleRate: 48000;
  loudnessTargetLufs: number;
  truePeakDbtp: number;
  defaultLanguage?: string;
}
export interface TransitionDefaults {
  defaultTake?: "cut" | "mix";
  mixDurationFrames?: number;
}
export interface OutputDefaults {
  record?: {
    container?: "fragmentedMp4" | "matroska";
    directory?: string;
    isolation?: {
      enabled?: boolean;
      tracks?: string[];
      [k: string]: unknown;
    };
  };
  stream?: {
    protocol?: "rtmp" | "srt" | "whip";
    videoBitrateKbps?: number;
    audioBitrateKbps?: number;
  };
  preview?: {
    enabled?: boolean;
    protocol?: "whep" | "mjpeg";
    path?: string;
  };
}
export interface Asset {
  id: Id;
  kind: "video" | "alphaVideo" | "audio" | "image" | "font" | "rss" | "wasm" | "wgsl";
  source: string;
  sha256?: string;
  format?:
    "h264" | "prores4444" | "hapAlpha" | "pngSequence" | "aac" | "pcm" | "wav" | "png" | "svg" | "ttf" | "otf" | "rss";
  cadence?: "preserve" | "interpolate";
  sourceFrameRate?: number;
  expectedDurationFrames?: number;
  pulldown?: Pulldown;
  loop?: LoopMetadata;
  loudness?: LoudnessReport;
}
export interface LoopMetadata {
  periodFrames: number;
  t0Frames?: number;
  seamless?: boolean;
  cachePolicy?: "auto" | "vram" | "stream";
  vramBudgetMib?: number;
  textureFormat?: "auto" | "nv12" | "rgba8" | "nv12Alpha" | "bc7";
}
export interface LoudnessReport {
  integratedLufs?: number;
  truePeakDbtp?: number;
  loudnessRange?: number;
  measuredBy?: string;
}
export interface GraphicTemplate {
  id: Id;
  kind: "lowerThirdHeadline" | "lowerThirdName" | "breakingBanner" | "ticker" | "generic";
  fontAssetIds?: Id[];
  fields?: {
    name: string;
    label?: string;
    multiline?: boolean;
    direction?: "auto" | "ltr" | "rtl";
    maxLength?: number;
  }[];
}
export interface Sequence {
  id: Id;
  title?: string;
  label?: string;
  /**
   * @minItems 1
   */
  items: [Item, ...Item[]];
}
export interface Control {
  bindings: ControlBinding[];
  companion?: {
    [k: string]: unknown;
  };
}
export interface ControlBinding {
  id: Id;
  description?: string;
  trigger?: {
    kind: "companionKey" | "hotkey" | "midi" | "webButton" | "osc";
    page?: number;
    bank?: number;
    key?: string;
  };
  action: string;
  payload?: {
    [k: string]: unknown;
  };
}
export interface Features {
  ndi?: {
    enabled?: boolean;
  };
}
export interface Scene {
  id: Id;
  name?: string;
  base?: Id;
  mergeMode?: "inherit" | "replace" | "merge";
  elements: Element[];
  audio?: {
    [k: string]: unknown;
  };
}
export interface Transform {
  x?: number;
  y?: number;
  w?: number;
  h?: number;
  crop?: {
    u?: number;
    v?: number;
    w?: number;
    h?: number;
  };
}
export interface ChromaKey {
  enabled: boolean;
  color?: "green" | "blue" | "custom";
  customColorHex?: string;
  tolerance?: number;
  softness?: number;
  spillSuppression?: number;
  edgeFeather?: number;
  garbageMatte?: Transform;
}
export interface LayerAudio {
  bus?: "clip" | "guest" | "sfx" | "music" | "mic";
  gainDb?: number;
  muted?: boolean;
}
export interface ClockConfig {
  mode?: "wall" | "showElapsed";
  timezone?: string;
  format?: "HH:mm" | "HH:mm:ss" | "hh:mm A" | "locale";
  locale?: string;
  blinkColon?: boolean;
}
export interface Animation {
  durationFrames?: number;
  delayFrames?: number;
  easing?: "linear" | "easeIn" | "easeOut" | "easeInOut" | "cubicBezier" | "spring";
  /**
   * @minItems 4
   * @maxItems 4
   */
  bezier?: [number, number, number, number];
}
export interface Overlay {
  id: Id;
  elements: Element[];
}
export interface TransitionPreset {
  id: Id;
  kind?: "cut" | "mix" | "wipe" | "sting" | "move" | "dve";
  durationFrames?: number;
  easing?: "linear" | "easeIn" | "easeOut" | "easeInOut" | "cubicBezier" | "spring";
  elementOverrides?: {
    [k: string]: unknown;
  };
}
export interface AutomationRule {
  id: Id;
  trigger: {
    kind:
      | "mediaEnd"
      | "mediaStart"
      | "timer"
      | "timeOfDay"
      | "audioLevel"
      | "hotkey"
      | "rssKeyword"
      | "streamHealth"
      | "stateChange";
    params?: {
      [k: string]: unknown;
    };
  };
  conditions?: {
    [k: string]: unknown;
  }[];
  action: {
    command: string;
    payload?: {
      [k: string]: unknown;
    };
  };
  enabled?: boolean;
}
export interface Plugin {
  id: Id;
  kind: "effect" | "element";
  source: string;
  version?: string;
  maxMemoryMib?: number;
  permissions?: ("network" | "disk" | "camera" | "microphone")[];
}
