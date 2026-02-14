<script setup lang="ts">
import { useI18n } from "vue-i18n";
import { type ChannelMixMode } from "../generated/bindings";
import CheckmarkIcon from "./icons/CheckmarkIcon.vue";
  
const { t } = useI18n();

const props = defineProps<{
  id: string;
  name: string;
  enabled: boolean;
  mixMode: ChannelMixMode;
}>();

const emit = defineEmits(["update:enabled", "update:mixMode"]);

const toggleEnabled = () => {
  emit("update:enabled", !props.enabled);
};

const updateMixMode = (e: Event) => {
  emit(
    "update:mixMode",
    (e.target as HTMLSelectElement).value as ChannelMixMode,
  );
};
</script>

<template>
  <div
    class="bg-[#111823] p-4 rounded-xl border border-white/5 flex items-center justify-between hover:border-white/10 transition-colors"
    :class="{ 'opacity-50': !enabled }"
  >
    <div class="flex items-center gap-4">
      <div
        @click="toggleEnabled"
        class="w-6 h-6 rounded-md border-2 cursor-pointer flex items-center justify-center transition-colors"
        :class="enabled ? 'bg-[#2bd97f] border-[#2bd97f]' : 'border-white/20'"
      >
        <CheckmarkIcon v-if="enabled" />
      </div>
      <div class="flex flex-col">
        <span class="text-[#eaeaea] font-medium">{{ name }}</span>
        <span class="text-[#8c8c8c] text-xs">{{ id }}</span>
      </div>
    </div>

    <div class="flex items-center gap-3">
      <select
        :disabled="!enabled"
        :value="mixMode"
        @change="updateMixMode"
        class="bg-[#0e141d] border border-white/5 p-2 rounded-lg text-sm outline-none focus:border-[#2bd97f]/50 transition-colors disabled:opacity-50 cursor-pointer"
      >
        <option value="Stereo" :title="t('mixModeTooltips.Stereo')">
          {{ t("mixModes.Stereo") }}
        </option>
        <option value="Left" :title="t('mixModeTooltips.Left')">
          {{ t("mixModes.Left") }}
        </option>
        <option value="Right" :title="t('mixModeTooltips.Right')">
          {{ t("mixModes.Right") }}
        </option>
        <option value="Center" :title="t('mixModeTooltips.Center')">
          {{ t("mixModes.Center") }}
        </option>
        <option value="FrontLeft" :title="t('mixModeTooltips.FrontLeft')">
          {{ t("mixModes.FrontLeft") }}
        </option>
        <option value="FrontRight" :title="t('mixModeTooltips.FrontRight')">
          {{ t("mixModes.FrontRight") }}
        </option>
        <option value="BackLeft" :title="t('mixModeTooltips.BackLeft')">
          {{ t("mixModes.BackLeft") }}
        </option>
        <option value="BackRight" :title="t('mixModeTooltips.BackRight')">
          {{ t("mixModes.BackRight") }}
        </option>
        <option value="BackSurround" :title="t('mixModeTooltips.BackSurround')">
          {{ t("mixModes.BackSurround") }}
        </option>
        <option value="Subwoofer" :title="t('mixModeTooltips.Subwoofer')">
          {{ t("mixModes.Subwoofer") }}
        </option>
      </select>
    </div>
  </div>
</template>
