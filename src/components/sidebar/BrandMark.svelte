<script lang="ts">
  // The app icon (design/herdr-desktop-v2.pen, "Ícone A · Painéis") drawn as SVG so the sidebar
  // head and the rail show the same mark as the bundle icons, crisp at any size.
  let { size = 24 }: { size?: number } = $props();
  const id = `brand-bg-${Math.random().toString(36).slice(2, 8)}`;
</script>

<svg class="brand-mark" data-brand-mark viewBox="0 0 1024 1024" width={size} height={size} aria-hidden="true">
  <defs>
    <linearGradient id={id} x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#262626" />
      <stop offset="1" stop-color="#0a0a0a" />
    </linearGradient>
  </defs>
  <rect x="2" y="2" width="1020" height="1020" rx="228" fill="url(#{id})" stroke="#ffffff" stroke-opacity="0.1" stroke-width="4" />
  <rect x="168" y="208" width="688" height="608" rx="72" fill="#171717" stroke="#ffffff" stroke-opacity="0.14" stroke-width="6" />
  <rect x="208" y="248" width="336" height="528" rx="44" fill="#242424" />
  <rect x="568" y="248" width="248" height="252" rx="44" fill="#242424" />
  <rect x="568" y="524" width="248" height="252" rx="44" fill="#242424" />
  <path d="M258 318 L352 380 L258 442" fill="none" stroke="#ededed" stroke-width="34" stroke-linecap="round" stroke-linejoin="round" />
  <rect x="372" y="424" width="104" height="30" rx="15" fill="#ededed" />
  <rect x="252" y="520" width="220" height="26" rx="13" fill="#6b6b6b" />
  <rect x="252" y="572" width="160" height="26" rx="13" fill="#4a4a4a" />
  <rect x="252" y="624" width="190" height="26" rx="13" fill="#4a4a4a" />
  <circle cx="692" cy="374" r="40" fill="#4ade80" />
  <circle cx="692" cy="650" r="40" fill="#e9a23b" />
</svg>

<style>
  .brand-mark {
    display: block;
    flex-shrink: 0;
  }
</style>
