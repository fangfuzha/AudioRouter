import { createI18n } from "vue-i18n";

import en from "./locales/en.json";
import zh from "./locales/zh.json";

export const messages = {
  en,
  zh,
};

export const i18n = createI18n({
  legacy: false, // use Composition API(Vue 3) for i18n
  globalInjection: true, // allow global $t function
  locale: "en", // default locale
  fallbackLocale: "en", // fallback locale
  messages, // set locale messages
});

export default i18n;