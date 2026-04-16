<script setup lang="ts">
interface Props {
  width?: number;
  height?: number;
  strokeWidth?: number;
}

withDefaults(defineProps<Props>(), {
  width: 18,
  height: 18,
  strokeWidth: 2.5,
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
    <circle cx="12" cy="12" r="1" />
    <circle cx="12" cy="5" r="1" />
    <circle cx="12" cy="19" r="1" />
  </svg>
</template>
