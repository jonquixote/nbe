//! Section 16.5 — element/graphic commands.

import { CpError } from "../protocol.js";
import type { CommandRegistry, DispatchDeps, HandlerOutput } from "../dispatch.js";

export function elementHandlers(reg: CommandRegistry, _deps: DispatchDeps): void {
  reg.set("element.toggle", {
    forward: true,
    handler: (ctx, payload): HandlerOutput => {
      const elementId = String(payload.elementId);
      ctx.state.requireElement(elementId);
      const existing = ctx.state.elementOverrides.get(elementId) ?? {};
      const visible = (payload.visible as boolean | undefined) ?? !(existing.visible as boolean | undefined ?? true);
      ctx.state.elementOverrides.set(elementId, { ...existing, visible });
      return { data: { elementId, visible } };
    },
  });

  reg.set("element.set", {
    forward: true,
    handler: (ctx, payload): HandlerOutput => {
      const elementId = String(payload.elementId);
      ctx.state.requireElement(elementId);
      const existing = ctx.state.elementOverrides.get(elementId) ?? {};
      ctx.state.elementOverrides.set(elementId, { ...existing, ...(payload.patch as object) });
      return {};
    },
  });

  reg.set("graphic.show", {
    forward: true,
    handler: (ctx, payload): HandlerOutput => {
      const templateId = String(payload.templateId);
      const pkg = ctx.state.requirePackage();
      if (!pkg.templates.has(templateId)) throw new CpError("E_NOT_FOUND", `no such template: ${templateId}`);
      const elementId = String(payload.elementId ?? `${templateId}-graphic`);
      ctx.state.graphics.set(elementId, {
        templateId,
        fields: (payload.fields as Record<string, unknown>) ?? {},
      });
      return { data: { elementId } };
    },
  });

  reg.set("graphic.hide", {
    forward: true,
    handler: (ctx, payload): HandlerOutput => {
      const elementId = payload.elementId as string | undefined;
      const templateId = payload.templateId as string | undefined;
      if (!elementId && !templateId) throw new CpError("E_BAD_PAYLOAD", "elementId or templateId required");
      let removed = false;
      for (const [id, g] of ctx.state.graphics) {
        if (id === elementId || (templateId && g.templateId === templateId)) {
          ctx.state.graphics.delete(id);
          removed = true;
        }
      }
      if (!removed) throw new CpError("E_NOT_FOUND", "graphic not found");
      return {};
    },
  });

  reg.set("graphic.update", {
    forward: true,
    handler: (ctx, payload): HandlerOutput => {
      const elementId = String(payload.elementId);
      const g = ctx.state.graphics.get(elementId);
      if (!g) throw new CpError("E_NOT_FOUND", `no active graphic: ${elementId}`);
      g.fields = { ...g.fields, ...(payload.fields as Record<string, unknown>) };
      return {};
    },
  });

  reg.set("breaking.show", {
    forward: true,
    handler: (ctx, payload): HandlerOutput => {
      const pkg = ctx.state.requirePackage();
      if (pkg.breakingTemplates.size === 0) {
        throw new CpError("E_NOT_FOUND", "no breaking template declared in this package");
      }
      ctx.state.breakingVisible = true;
      ctx.state.breakingFields = payload.subhead !== undefined
        ? { headline: String(payload.headline), subhead: String(payload.subhead) }
        : { headline: String(payload.headline) };
      return {};
    },
  });

  reg.set("breaking.hide", {
    forward: true,
    handler: (ctx): HandlerOutput => {
      ctx.state.breakingVisible = false;
      ctx.state.breakingFields = null;
      return {};
    },
  });
}
