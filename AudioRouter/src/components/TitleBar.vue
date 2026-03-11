<script setup lang="ts">
import { ref } from "vue";
import { getCurrentWindow } from "@tauri-apps/api/window";
import MinusIcon from "./icons/MinusIcon.vue";
import MaximizeIcon from "./icons/MaximizeIcon.vue";
import RestoreIcon from "./icons/RestoreIcon.vue";
import CloseIcon from "./icons/CloseIcon.vue";

const appWindow = getCurrentWindow();
const isMaximized = ref(false);

async function checkMaximized() {
  isMaximized.value = await appWindow.isMaximized();
}

checkMaximized();

async function handleMinimize() {
  await appWindow.minimize();
}

async function handleMaximize() {
  if (isMaximized.value) {
    await appWindow.unmaximize();
  } else {
    await appWindow.maximize();
  }
  await checkMaximized();
}

async function handleClose() {
  await appWindow.close();
}

async function startDrag() {
  await appWindow.startDragging();
}
</script>

<template>
  <div
    class="h-10 flex items-center justify-between select-none border-b border-white/5"
    @mousedown="startDrag"
    style="background: var(--bg-primary)"
  >
    <div class="flex items-center gap-2 px-4">
      <span class="text-sm font-medium" style="color: var(--text-primary)"
        >AudioRouter</span
      >
    </div>

    <div class="flex items-center" @mousedown.stop>
      <button @click="handleMinimize" class="title-btn minimize">
        <MinusIcon :width="14" :height="14" />
      </button>
      <button @click="handleMaximize" class="title-btn maximize">
        <MaximizeIcon v-if="!isMaximized" :width="14" :height="14" />
        <RestoreIcon v-else :width="14" :height="14" />
      </button>
      <button @click="handleClose" class="title-btn close">
        <CloseIcon :width="14" :height="14" />
      </button>
    </div>
  </div>
</template>

<style scoped>
.title-btn {
  all: unset; /* 重置样式 */
  cursor: pointer;
  width: 36px;
  height: 36px;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--text-muted);
  transition: all 0.2s;
}

.title-btn.minimize:hover,
.title-btn.maximize:hover {
  background-color: var(--bg-tertiary);
}

.title-btn.close:hover {
  background-color: var(--accent-red);
  color: white;
}
</style>
