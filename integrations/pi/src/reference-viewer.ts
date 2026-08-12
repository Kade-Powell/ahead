import { getMarkdownTheme, type ExtensionCommandContext } from "@earendil-works/pi-coding-agent";
import { Markdown, matchesKey, truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";

export async function showReferenceViewer(
  ctx: ExtensionCommandContext,
  title: string,
  content: string,
): Promise<void> {
  await ctx.ui.custom<void>((tui, theme, _keybindings, done) => {
    const markdown = new Markdown(content.trim(), 0, 0, getMarkdownTheme());
    let scrollOffset = 0;
    let pageSize = 10;
    let totalLines = 0;

    const component = {
      render(width: number): string[] {
        const innerWidth = Math.max(20, width - 2);
        const contentWidth = Math.max(18, innerWidth - 2);
        const rendered = markdown.render(contentWidth);
        totalLines = rendered.length;
        pageSize = Math.max(6, Math.min(30, tui.terminal.rows - 7));
        const maxOffset = Math.max(0, totalLines - pageSize);
        scrollOffset = Math.min(scrollOffset, maxOffset);
        const visible = rendered.slice(scrollOffset, scrollOffset + pageSize);
        const remaining = Math.max(0, totalLines - pageSize - scrollOffset);
        const border = (value: string) => theme.fg("border", value);
        const pad = (value: string): string => {
          const truncated = truncateToWidth(value, innerWidth, "…", true);
          return `${truncated}${" ".repeat(Math.max(0, innerWidth - visibleWidth(truncated)))}`;
        };
        const lines = [
          border(`╭${"─".repeat(innerWidth)}╮`),
          `${border("│")}${pad(` ${theme.fg("accent", theme.bold(title))}`)}${border("│")}`,
          `${border("│")}${pad(theme.fg("dim", ` ↑ ${scrollOffset} lines · ↓ ${remaining} lines`))}${border("│")}`,
        ];
        for (const line of visible) {
          lines.push(`${border("│")}${pad(` ${line}`)}${border("│")}`);
        }
        for (let index = visible.length; index < pageSize; index += 1) {
          lines.push(`${border("│")}${pad("")}${border("│")}`);
        }
        lines.push(
          `${border("│")}${pad(theme.fg("dim", " ↑↓ scroll · PgUp/PgDn page · Home/End jump · Esc close"))}${border("│")}`,
          border(`╰${"─".repeat(innerWidth)}╯`),
        );
        return lines;
      },
      invalidate(): void {
        markdown.invalidate();
      },
      handleInput(data: string): void {
        const maxOffset = Math.max(0, totalLines - pageSize);
        if (matchesKey(data, "escape") || matchesKey(data, "ctrl+c")) {
          done();
        } else if (matchesKey(data, "up")) {
          scrollOffset = Math.max(0, scrollOffset - 1);
        } else if (matchesKey(data, "down")) {
          scrollOffset = Math.min(maxOffset, scrollOffset + 1);
        } else if (matchesKey(data, "pageUp")) {
          scrollOffset = Math.max(0, scrollOffset - pageSize);
        } else if (matchesKey(data, "pageDown")) {
          scrollOffset = Math.min(maxOffset, scrollOffset + pageSize);
        } else if (matchesKey(data, "home")) {
          scrollOffset = 0;
        } else if (matchesKey(data, "end")) {
          scrollOffset = maxOffset;
        }
        tui.requestRender();
      },
    };
    return component;
  });
}
