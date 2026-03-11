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
    <rect x="5" y="5" width="14" height="14" rx="2" ry="2"></rect>
  </svg>
</template>
