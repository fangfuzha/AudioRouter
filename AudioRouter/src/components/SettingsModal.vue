<script setup lang="ts">
import { ref, onMounted } from "vue";
import { commands, type General } from "../generated/bindings";
import { useI18n } from "vue-i18n";
import CloseIcon from "./icons/CloseIcon.vue";
import { unwrap } from "../utils/ipc";
import { updateAppLanguage, availableLanguages } from "../utils/language";
const { locale, t } = useI18n();

// props
const props = defineProps<{
  show: boolean;
}>();
const emit = defineEmits(["close"]);

// draftGeneral = editable copy of general settings shown in the modal;
// changes here DO NOT take effect until the user clicks "保存设置" (save)
const draftGeneral = ref<General>({} as General);

async function loadGeneralConfig() {
  try {
    const config = await unwrap(commands.getConfig());
    // Load only the general settings
    draftGeneral.value = { ...config.general };
  } catch (e) {
    console.error("Failed to load config:", e);
  }
}

// Saves the general settings and updates the app language if it was changed
async function updateGeneralConfig() {
  try {
    // Save only the general settings using the new API
    await unwrap(commands.updateGeneralConfig(draftGeneral.value));

    // Update locale and tray menu after successful save
    await updateAppLanguage(draftGeneral.value.language || "en");

    emit("close");
  } catch (e) {
    console.error("Failed to save config:", e);
  }
}

onMounted(() => {
  loadGeneralConfig();
});
</script>

<template>
  <div
    v-if="show"
    class="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm"
    @click.self="emit('close')"
  >
    <div
      class="w-80 bg-[#0e141d] border border-white/10 rounded-2xl shadow-2xl p-6 flex flex-col gap-6"
    >
      <div class="flex items-center justify-between">
        <h3 class="text-lg font-bold text-white">{{ t("SettingsTitle") }}</h3>
        <button
          @click="emit('close')"
          class="text-[#8c8c8c] hover:text-white transition-colors"
        >
          <CloseIcon />
        </button>
      </div>

      <div class="flex flex-col gap-4">
        <label class="flex items-center justify-between cursor-pointer group">
          <span
            class="text-[#eaeaea] group-hover:text-white transition-colors"
            >{{ t("StartWithWindows") }}</span
          >
          <input
            type="checkbox"
            v-model="draftGeneral.start_with_windows"
            class="w-5 h-5 rounded border-white/10 bg-[#0b0f14] checked:bg-[#2bd97f] focus:ring-0 transition-all cursor-pointer"
          />
        </label>

        <label class="flex items-center justify-between cursor-pointer group">
          <span
            class="text-[#eaeaea] group-hover:text-white transition-colors"
            >{{ t("StartMinimized") }}</span
          >
          <input
            type="checkbox"
            v-model="draftGeneral.minimized"
            class="w-5 h-5 rounded border-white/10 bg-[#0b0f14] checked:bg-[#2bd97f] focus:ring-0 transition-all cursor-pointer"
          />
        </label>

        <label class="flex items-center justify-between cursor-pointer group">
          <span
            class="text-[#eaeaea] group-hover:text-white transition-colors"
            >{{ t("AutoRoute") }}</span
          >
          <input
            type="checkbox"
            v-model="draftGeneral.auto_route"
            class="w-5 h-5 rounded border-white/10 bg-[#0b0f14] checked:bg-[#2bd97f] focus:ring-0 transition-all cursor-pointer"
          />
        </label>

        <label class="flex items-center justify-between cursor-pointer group">
          <span
            class="text-[#eaeaea] group-hover:text-white transition-colors"
            >{{ t("Language") }}</span
          >
          <select
            v-model="draftGeneral.language"
            class="bg-[#0e141d] border border-white/5 p-2 rounded-lg text-sm outline-none focus:border-[#2bd97f]/50 transition-colors cursor-pointer"
          >
            <option
              v-for="l in availableLanguages"
              :key="l.code"
              :value="l.code"
            >
              {{ l.name }}
            </option>
          </select>
        </label>
      </div>

      <button
        @click="updateGeneralConfig"
        class="mt-2 w-full bg-[#2bd97f] hover:bg-[#24b86b] text-[#0b0f14] font-bold py-3 rounded-xl transition-all shadow-lg shadow-[#2bd97f]/20"
      >
        {{ t("Save") }}
      </button>
    </div>
  </div>
</template>
