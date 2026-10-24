/**
 * Keyboard map: turns a keydown into a named action, given what has focus.
 *
 * Kept pure so the precedence rules are tested rather than remembered. The one
 * that matters most is Esc: it closes the modal, leaves a composer or field
 * (so a filter survives while you type a reply), clears the search, blurs
 * whatever has focus (a button keeps focus after a click), and only with none
 * of those left does it dismiss a live prompt in the pane — a stray Esc must
 * never reach a real session.
 *
 * ctx.focus: "none" | "button" | "search" | "composer" | "field" (any other input).
 * ctx.armed: false when the selection moved to this session by itself (the
 * one the user was on left the list); digits and Esc don't answer its prompt
 * until the user selects it.
 */

export function keyAction(e, ctx) {
  const cmd = e.metaKey && !e.ctrlKey && !e.altKey;
  if (cmd && e.key === ",") return "openSettings";
  if (ctx.modalOpen) return e.key === "Escape" ? "closeModal" : null;
  if (cmd && (e.key === "r" || e.key === "R")) return "refresh";
  if (cmd && e.key === "w") return "hide";
  if (cmd && e.key === "Enter") return ctx.focus === "composer" ? "send" : null;
  if (e.metaKey || e.ctrlKey || e.altKey) return null;

  if (e.key === "Escape") {
    // Typing a reply to a filtered session shouldn't lose the filter on Esc.
    if (ctx.focus === "composer" || ctx.focus === "field") return "blur";
    if (ctx.searchHasText) return "clearSearch";
    if (ctx.focus !== "none") return "blur";
    return ctx.armed && ctx.needsInput && ctx.optionCount > 0 ? "dismiss" : null;
  }

  // A focused button (an option, Dismiss, Jump) takes its own activation keys.
  if (ctx.focus === "button" && (e.key === "Enter" || e.key === " ")) return null;

  const typing = ctx.focus === "composer" || ctx.focus === "field";
  if (typing) return null;

  // From the search box the arrows and Enter still drive the list, the way a
  // command palette behaves; letters belong to the query.
  if (e.key === "ArrowDown") return "next";
  if (e.key === "ArrowUp") return "prev";
  if (e.key === "Enter") return "jump";
  if (ctx.focus === "search") return null;

  if (e.key === "j") return "next";
  if (e.key === "k") return "prev";
  if (e.key === "/") return "focusSearch";
  if (e.key === "g") return "toggleGroup";
  if (e.key === "r") return "focusComposer";
  if (/^[1-9]$/.test(e.key) && ctx.armed && ctx.needsInput && Number(e.key) <= ctx.optionCount) return `choose:${e.key}`;
  return null;
}
