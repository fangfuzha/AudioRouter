<script setup lang="ts">
interface Props {
  width?: number;
  height?: number;
  strokeWidth?: number;
}

withDefaults(defineProps<Props>(), {
  width: 20,
  height: 20,
  strokeWidth: 2,
});
</script>

<template>
  <svg
    :width="width"
    :height="height"
    xmlns="http://www.w3.org/2000/svg"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    :stroke-width="strokeWidth"
    stroke-linecap="round"
    stroke-linejoin="round"
  >
    <rect x="5" y="9" width="10" height="10" rx="1"></rect>
    <path d="M9 9V5a1 1 0 0 1 1-1h9a1 1 0 0 1 1 1v9a1 1 0 0 1-1 1h-4"></path>
  </svg>
</template>
