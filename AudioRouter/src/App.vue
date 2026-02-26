<script setup lang="ts">
import { ref, onMounted, onUnmounted, watch, computed } from "vue";
import { listen } from "@tauri-apps/api/event";
import { useI18n } from "vue-i18n";
import { checkForUpdates } from "./utils/update";
import {
  commands,
  type ChannelMixMode,
  type UiDataTargetDevice,
  type Output,
} from "./generated/bindings";
import DeviceCard from "./components/DeviceCard.vue";
import SettingsModal from "./components/SettingsModal.vue";
import RefreshIcon from "./components/icons/RefreshIcon.vue";
import SettingsIcon from "./components/icons/SettingsIcon.vue";
import PlayIcon from "./components/icons/PlayIcon.vue";
import StopIcon from "./components/icons/StopIcon.vue";
import { unwrap } from "./utils/ipc";
const { t } = useI18n();

const selectedSource = ref("");
const target_devices = ref<UiDataTargetDevice[]>([]);

// 按设备名称排序（仅用于显示，不修改底层 `target_devices` 的顺序）
const sorted_target_devices = computed<UiDataTargetDevice[]>(() =>
  [...target_devices.value]
    .slice()
    .sort((a, b) => a.name.localeCompare(b.name)),
);

// 用于渲染目标设备列表的计算属性，会排除当前选中的源设备
const filtered_target_devices = computed<UiDataTargetDevice[]>(() =>
  sorted_target_devices.value.filter((d) => d.id !== selectedSource.value),
);

const isRunning = ref(false);
const statusText = ref(t("StatusReady"));
const showSettings = ref(false);

async function saveRoutingConfig() {
  try {
    const outputs: Output[] = target_devices.value.map((d) => ({
      device_id: d.id,
      channel_mode: d.mix_mode,
      enabled: d.enabled,
    }));
    await unwrap(commands.saveRoutingConfig(selectedSource.value, outputs));
  } catch (e) {
    console.error("Failed to save routing config:", e);
  }
}

async function refreshUI() {
  try {
    const uiData = await unwrap(commands.getUiData());

    target_devices.value = uiData.target_devices;
    isRunning.value = uiData.is_running;

    if (
      uiData.source_device &&
      uiData.target_devices.some((d) => d.id === uiData.source_device)
    ) {
      selectedSource.value = uiData.source_device;
    } else if (target_devices.value.length > 0 && !selectedSource.value) {
      // 默认选择按名称排序后的第一个设备，保证展示与默认值一致
      selectedSource.value = sorted_target_devices.value[0].id;
    }
    statusText.value = t("FoundDevices", {
      count: target_devices.value.length,
    });
  } catch (e) {
    statusText.value = t("ErrorLoadingDevices");
  }
}

async function startRouting() {
  const targets = filtered_target_devices.value
    .filter((d) => d.enabled)
    .map((d) => [d.id, d.mix_mode] as [string, ChannelMixMode]);

  if (targets.length === 0) {
    statusText.value = t("SelectDevice");
    return;
  }

  try {
    statusText.value = t("Starting");
    await unwrap(
      commands.startRouting({
        source_id: selectedSource.value,
        targets: targets,
      }),
    );
    // 等待后端事件来更新状态
  } catch (e) {
    statusText.value = `Error starting: ${e}`;
  }
}

async function stopRouting() {
  try {
    statusText.value = t("Stopping");
    await unwrap(commands.stopRouting());
    // 等待后端事件来更新状态
  } catch (e) {
    statusText.value = `Error stopping: ${e}`;
  }
}

let unlistenStart: any = null;
let unlistenStop: any = null;

onMounted(async () => {
  await refreshUI();
  await checkForUpdates();

  // 监听路由配置变化并自动保存
  watch(() => selectedSource.value, saveRoutingConfig);
  watch(() => target_devices.value, saveRoutingConfig, { deep: true });

  // listen to routing start/stop events from backend
  unlistenStart = await listen("routing-started", () => {
    isRunning.value = true;
    const runningCount = filtered_target_devices.value.filter(
      (d) => d.enabled,
    ).length;
    statusText.value = t("RunningOn", { count: runningCount });
  });
  unlistenStop = await listen("routing-stopped", () => {
    isRunning.value = false;
    statusText.value = t("StatusReady");
  });
});

onUnmounted(() => {
  // cleanup listeners on unmount
  if (unlistenStart) unlistenStart.then((u: any) => u());
  if (unlistenStop) unlistenStop.then((u: any) => u());
});
</script>

<template>
  <div
    class="h-170 w-225 bg-[#0b0f14] text-[#eaeaea] flex flex-col overflow-hidden border border-white/10 rounded-xl"
  >
    <div class="flex-1 p-8 overflow-hidden flex flex-col gap-6">
      <!-- Source Section -->
      <div class="flex flex-col gap-3 text-left">
        <label
          class="text-[#8c8c8c] text-[10px] font-bold tracking-widest uppercase"
          >{{ t("SourceDevice") }}</label
        >
        <select
          v-model="selectedSource"
          class="bg-[#0e141d] border border-white/5 p-3 rounded-xl outline-none focus:border-[#2bd97f]/50 transition-colors cursor-pointer"
        >
          <option v-for="d in sorted_target_devices" :key="d.id" :value="d.id">
            {{ d.name }}
          </option>
        </select>
      </div>

      <!-- Output Section -->
      <div class="flex flex-col gap-3 flex-1 overflow-hidden text-left">
        <label
          class="text-[#8c8c8c] text-[10px] font-bold tracking-widest uppercase"
          >{{ t("OutputDevices") }}</label
        >
        <div
          class="flex-1 overflow-y-auto pr-2 flex flex-col gap-3 custom-scrollbar"
        >
          <DeviceCard
            v-for="d in filtered_target_devices"
            :key="d.id"
            :id="d.id"
            :name="d.name"
            v-model:enabled="d.enabled"
            v-model:mix-mode="d.mix_mode"
          />
        </div>
      </div>
    </div>

    <!-- Bottom Bar -->
    <div
      class="h-20 bg-[#0b0f14] border-t border-white/5 flex items-center justify-between px-8"
    >
      <div class="flex items-center gap-2">
        <div
          class="w-2 h-2 rounded-full transition-all"
          :class="
            isRunning
              ? 'bg-[#00ff77] shadow-[0_0_8px_#2bd97f] animate-pulse'
              : 'bg-[#ff0000]'
          "
        ></div>
        <div class="text-[#b3b3b3] text-sm">{{ statusText }}</div>
      </div>

      <div class="flex items-center gap-2">
        <button
          @click="refreshUI"
          class="p-3 text-[#b3b3b3] hover:bg-[#111823] rounded-xl transition-colors"
          :title="t('RefreshDevices')"
        >
          <RefreshIcon />
        </button>
        <button
          @click="showSettings = true"
          class="p-3 text-[#b3b3b3] hover:bg-[#111823] rounded-xl transition-colors"
          :title="t('Settings')"
        >
          <SettingsIcon />
        </button>

        <!-- Separator -->
        <div class="w-px h-8 bg-white/5 mx-2"></div>

        <button
          v-if="!isRunning"
          @click="startRouting"
          class="bg-[#2bd97f] hover:bg-[#23c86e] text-[#0b0f14] px-8 h-11 rounded-xl font-bold transition-all flex items-center gap-2"
        >
          <PlayIcon />
          {{ t("Start") }}
        </button>
        <button
          v-else
          @click="stopRouting"
          class="bg-[#ff4d4d] hover:bg-[#e63c3c] text-white px-8 h-11 rounded-xl font-bold transition-all flex items-center gap-2 shadow-[0_4px_12px_rgba(255,77,77,0.2)]"
        >
          <StopIcon />
          {{ t("Stop") }}
        </button>
      </div>
    </div>

    <SettingsModal :show="showSettings" @close="showSettings = false" />
  </div>
</template>
