<script setup lang="ts">
interface Props {
  width?: number;
  height?: number;
  strokeWidth?: number;
  color?: string;
}

withDefaults(defineProps<Props>(), {
  width: 16,
  height: 16,
  strokeWidth: 3,
  color: "#0b0f14",
});
</script>

<template>
  <svg
    :width="width"
    :height="height"
    xmlns="http://www.w3.org/2000/svg"
    viewBox="0 0 24 24"
    fill="none"
    :stroke="color"
    :stroke-width="strokeWidth"
    stroke-linecap="round"
    stroke-linejoin="round"
  >
    <polyline points="20 6 9 17 4 12"></polyline>
  </svg>
</template>
