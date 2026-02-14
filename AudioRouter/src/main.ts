import { createApp } from "vue";
import "./assets/main.css";
import App from "./App.vue";
import i18n from "./i18n";
import { commands } from "./generated/bindings";
import { unwrap } from "./utils/ipc";
import { updateAppLanguage } from "./utils/language";

// Initialize locale and tray menu from persisted config
(async () => {
  try {
    const lang = await unwrap(commands.getLanguage());
    await updateAppLanguage(lang || "en");
  } catch (e) {
    console.error("Failed to initialize language:", e);
  }
})();

const app = createApp(App);
app.use(i18n);
app.mount("#app");
