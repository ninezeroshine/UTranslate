const PATHS: Record<string, string[]> = {
  arrow: ["M2.5 7h9", "M8 3.5L11.5 7 8 10.5"],
  chevron: ["M3 4.5l3 3 3-3"],
  close: ["M4 4l8 8", "M12 4l-8 8"],
  pin: ["M6 2.5h4l-.5 4 2 1.5V9H4.5V8l2-1.5z", "M8 9v4.5"],
  expand: ["M9.5 2.5h4v4", "M13.5 2.5L9 7", "M6.5 13.5h-4v-4", "M2.5 13.5L7 9"],
  copy: ["M5.5 5.5h7.5v7.5H5.5z", "M10.5 5.5V3.5a1 1 0 0 0-1-1h-6a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2"],
  speaker: ["M2.5 6.5h2.5l3.5-3v9l-3.5-3H2.5z", "M11 5.5a3.5 3.5 0 0 1 0 5", "M12.8 3.6a6 6 0 0 1 0 8.8"],
  star: ["M8 2.2l1.8 3.7 4 .6-2.9 2.8.7 4L8 11.4l-3.6 1.9.7-4L2.2 6.5l4-.6z"],
  swap: ["M3 5.5h9", "M9.5 3l2.5 2.5L9.5 8", "M13 10.5H4", "M6.5 8L4 10.5 6.5 13"],
  minimize: ["M3 8h10"],
  maximize: ["M3.5 3.5h9v9h-9z"],
  search: ["M7 11.5a4.5 4.5 0 1 0 0-9 4.5 4.5 0 0 0 0 9z", "M10.5 10.5L14 14"],
  trash: ["M3 4.5h10", "M6.5 4.5v-2h3v2", "M4.5 4.5l.7 8.5h5.6l.7-8.5"],
  check: ["M3 8.5l3 3 7-7"],
  refresh: ["M13 8a5 5 0 1 1-1.5-3.5", "M13 2.5v3h-3"],
  translate: ["M2.5 4h7", "M6 2.5V4", "M8 4c-.5 3-2.5 5.5-5 6.5", "M4.5 6c.8 2 2.5 3.5 4.5 4.5", "M9 14l2.5-6 2.5 6", "M10 12h3"],
};

export function Icon({ name, size = 16, className = "" }: { name: keyof typeof PATHS; size?: number; className?: string }) {
  const s = size / 16;
  return (
    <svg width={size} height={size} className={`ic ${className}`} style={{ flexShrink: 0 }}>
      <g transform={s === 1 ? undefined : `scale(${s})`}>
        {PATHS[name].map((d) => (
          <path key={d} d={d} />
        ))}
      </g>
    </svg>
  );
}
