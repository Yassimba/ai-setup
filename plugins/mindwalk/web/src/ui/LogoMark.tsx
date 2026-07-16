// The mark tells the product's story in one tile: a repository waiting in the
// dark, and an agent's walk lighting it up — footsteps of warm firefly light,
// touched nodes glowing by state (search moss, read moon, edit amber).
// Colors are literal so the mark stays identical to the favicon and README asset.
export function LogoMark({ size = 22 }: { size?: number }) {
  return (
    <svg viewBox="0 0 64 64" width={size} height={size} aria-hidden focusable="false">
      <defs>
        <radialGradient id="logo-halo">
          <stop offset="0%" stopColor="#f1aa57" stopOpacity="0.55" />
          <stop offset="100%" stopColor="#f1aa57" stopOpacity="0" />
        </radialGradient>
      </defs>
      <rect width="64" height="64" rx="14" fill="#0f141b" />
      <circle cx="20" cy="15" r="1.6" fill="#3b4047" />
      <circle cx="11" cy="27" r="1.4" fill="#3b4047" />
      <circle cx="51" cy="40" r="1.6" fill="#3b4047" />
      <circle cx="34" cy="53" r="1.4" fill="#3b4047" />
      <g fill="#f1aa57">
        <circle cx="18.6" cy="47.9" r="1.15" opacity="0.35" />
        <circle cx="22.7" cy="45.2" r="1.25" opacity="0.42" />
        <circle cx="25.9" cy="42" r="1.35" opacity="0.49" />
        <circle cx="28.2" cy="38.1" r="1.45" opacity="0.56" />
        <circle cx="30.9" cy="29.3" r="1.55" opacity="0.64" />
        <circle cx="33" cy="25.4" r="1.65" opacity="0.72" />
        <circle cx="36" cy="22.1" r="1.75" opacity="0.8" />
        <circle cx="38" cy="20.4" r="1.85" opacity="0.88" />
      </g>
      <circle cx="13" cy="50" r="4.5" fill="#9fc67b" opacity="0.16" />
      <circle cx="13" cy="50" r="2.4" fill="#9fc67b" />
      <circle cx="29.5" cy="34" r="5" fill="#b6daf1" opacity="0.16" />
      <circle cx="29.5" cy="34" r="2.7" fill="#b6daf1" />
      <circle cx="44" cy="17.5" r="9.5" fill="url(#logo-halo)" />
      <circle cx="44" cy="17.5" r="3.4" fill="#f1aa57" />
      <circle cx="44" cy="17.5" r="1.7" fill="#ffe0b0" />
    </svg>
  );
}
