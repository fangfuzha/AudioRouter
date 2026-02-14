import { unwrap } from "./ipc";
import { commands } from "../generated/bindings";
import i18n from "../i18n";
import languages from "../i18n/languages.json";

export const availableLanguages = Object.entries(languages).map(
  ([code, name]) => ({
    code,
    name,
  }),
);

/**
 * 更新应用语言设置（包括 i18n locale 和托盘菜单标签）
 *
 * @param language - 语言代码（如 "en", "zh"）
 *
 * @example
 * ```typescript
 * await updateAppLanguage("zh");
 * ```
 */
export async function updateAppLanguage(language: string): Promise<void> {
  // Update i18n locale
  i18n.global.locale.value = language as any;

  // Update tray menu labels to match the new language
  const labels = {
    show: i18n.global.t("Show"),
    quit: i18n.global.t("Exit"),
  };

  await unwrap(commands.updateTrayMenu(labels));
}
