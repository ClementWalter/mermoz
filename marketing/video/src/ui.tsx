/**
 * Visual building blocks for the launch video: Mermoz's very light blue ground, Liquid
 * Glass panels, and a recreation of the app's window, rows and chips. All content is
 * fictional demo data, never a real mailbox.
 */
import React, { createContext, useContext } from "react";
import { AbsoluteFill, interpolate, spring, useCurrentFrame, useVideoConfig } from "remotion";

export const C = {
  ground: "#EAF3FB",
  text: "#0D2436",
  text2: "#4B6477",
  text3: "#7A93A6",
  accent: "#1F6FB5",
  accentSoft: "#DCEBFA",
  line: "#DDE9F3",
  ok: "#2E9A6B",
  orange: "#E0782F",
  purple: "#8A6FD1",
};

/** Size of the product-shot stage: wide for 16:9, tall for 9:16. */
export const StageContext = createContext({ w: 1600, h: 880, portrait: false });
export const useStage = () => useContext(StageContext);

export const FONT = '-apple-system, "SF Pro Display", "Helvetica Neue", system-ui, sans-serif';

/** Soft drifting light behind everything. */
export const Ground: React.FC = () => {
  const f = useCurrentFrame();
  const blob = (x: number, y: number, size: number, color: string, speed: number, phase: number) => (
    <div
      style={{
        position: "absolute",
        left: `${x + Math.sin(f / speed + phase) * 6}%`,
        top: `${y + Math.cos(f / speed + phase) * 5}%`,
        width: size,
        height: size,
        borderRadius: "50%",
        background: color,
        filter: "blur(120px)",
        opacity: 0.55,
      }}
    />
  );
  return (
    <AbsoluteFill style={{ background: `linear-gradient(160deg, #F4F9FE 0%, ${C.ground} 55%, #DDEBF8 100%)`, overflow: "hidden" }}>
      {blob(-10, -15, 900, "#CFE3FA", 90, 0)}
      {blob(60, 40, 800, "#E3DAFB", 110, 2)}
      {blob(20, 70, 700, "#D4F0EA", 130, 4)}
    </AbsoluteFill>
  );
};

/** Frosted glass surface, like the app's panels. */
export const Glass: React.FC<{ style?: React.CSSProperties; radius?: number; children?: React.ReactNode }> = ({ style, radius = 22, children }) => (
  <div
    style={{
      background: "rgba(255,255,255,0.62)",
      backdropFilter: "blur(30px) saturate(1.4)",
      border: "1.5px solid rgba(255,255,255,0.9)",
      borderRadius: radius,
      boxShadow: "0 30px 80px rgba(31,111,181,0.14), 0 2px 6px rgba(13,36,54,0.05)",
      ...style,
    }}
  >
    {children}
  </div>
);

/** Spring from 0 to 1 starting at `delay` frames. */
export const useIn = (delay = 0, damping = 16) => {
  const f = useCurrentFrame();
  const { fps } = useVideoConfig();
  return spring({ frame: f - delay, fps, config: { damping, mass: 0.7 } });
};

/** Fade + lift for a scene's first and last frames. */
export const SceneFade: React.FC<{ duration: number; children: React.ReactNode }> = ({ duration, children }) => {
  const f = useCurrentFrame();
  const o = interpolate(f, [0, 12, duration - 12, duration], [0, 1, 1, 0], { extrapolateLeft: "clamp", extrapolateRight: "clamp" });
  const y = interpolate(f, [0, 14], [24, 0], { extrapolateRight: "clamp" });
  return <AbsoluteFill style={{ opacity: o, transform: `translateY(${y}px)` }}>{children}</AbsoluteFill>;
};

export const Chip: React.FC<{ children: React.ReactNode; ai?: boolean; style?: React.CSSProperties }> = ({ children, ai, style }) => (
  <span
    style={{
      display: "inline-flex",
      alignItems: "center",
      gap: 8,
      height: 34,
      padding: "0 14px",
      borderRadius: 10,
      fontSize: 19,
      fontWeight: 500,
      color: ai ? C.accent : C.text2,
      background: ai ? C.accentSoft : "rgba(255,255,255,0.95)",
      border: ai ? "none" : `1.5px solid ${C.line}`,
      whiteSpace: "nowrap",
      ...style,
    }}
  >
    {ai && <span style={{ fontSize: 16 }}>✦</span>}
    {children}
  </span>
);

export const Dot: React.FC<{ color: string; size?: number }> = ({ color, size = 12 }) => (
  <span style={{ width: size, height: size, borderRadius: size / 3, background: color, display: "inline-block" }} />
);

export type Mail = { n: string; from: string; subject: string; snippet: string; sort: string; project?: string; color?: string };

export const MAILS: Mail[] = [
  { n: "M-1287", from: "Marc Petit", subject: "Q4 audit — can you sign by Friday?", snippet: "Two pages, I flagged the parts that need you.", sort: "To do", project: "Acme", color: C.orange },
  { n: "M-1288", from: "Lisbon Air", subject: "Your boarding pass · TP 431", snippet: "Gate opens 45 minutes before departure.", sort: "Archive", project: "Travel", color: "#2F8FCF" },
  { n: "M-1289", from: "Léa Martin", subject: "Dinner on Saturday?", snippet: "We found a tiny place near the river.", sort: "To do", project: "Family", color: C.purple },
  { n: "M-1290", from: "Stripe", subject: "Receipt for your payment", snippet: "Amount paid €24.00 · Pro plan.", sort: "Archive", project: "Receipts", color: C.ok },
  { n: "M-1291", from: "Hugo · Design", subject: "New mockups attached", snippet: "Three directions, I like the second one.", sort: "In progress", project: "Website", color: "#C2568C" },
  { n: "M-1292", from: "The Weekly", subject: "Five things worth reading", snippet: "This week: calm software and slow mornings.", sort: "Archive", color: "#8FA3B5" },
];

/** One thread row in the list. `chip` is 0..1 for the AI suggestion pop. */
export const Row: React.FC<{ mail: Mail; unread?: boolean; chip?: number; selected?: boolean; style?: React.CSSProperties }> = ({
  mail,
  unread = true,
  chip = 0,
  selected,
  style,
}) => (
  <div
    style={{
      display: "flex",
      flexDirection: "column",
      gap: 6,
      padding: "18px 30px",
      borderTop: `1.5px solid ${C.line}`,
      background: selected ? "rgba(31,111,181,0.10)" : "transparent",
      boxShadow: selected ? `inset 5px 0 0 ${C.accent}` : "none",
      ...style,
    }}
  >
    <div style={{ display: "flex", alignItems: "center", gap: 14 }}>
      <span style={{ width: 12, height: 12, borderRadius: 6, background: unread ? C.accent : "transparent" }} />
      <span style={{ fontFamily: "ui-monospace, Menlo", fontSize: 17, color: C.text3 }}>{mail.n}</span>
      <span style={{ fontSize: 23, fontWeight: unread ? 650 : 450, color: C.text }}>{mail.from}</span>
      <span style={{ flex: 1 }} />
      {chip > 0.01 && (
        <span style={{ transform: `scale(${0.6 + 0.4 * chip})`, opacity: chip, transformOrigin: "right center" }}>
          <Chip ai>
            {mail.sort}
            {mail.project ? ` · ${mail.project}` : ""}
          </Chip>
        </span>
      )}
    </div>
    <div style={{ fontSize: 22, fontWeight: unread ? 600 : 400, color: unread ? C.text : C.text2, marginLeft: 26 }}>{mail.subject}</div>
    <div style={{ fontSize: 19, color: C.text2, marginLeft: 26 }}>{mail.snippet}</div>
  </div>
);

const STATUSES: [string, string][] = [
  ["Unread", C.accent],
  ["To do", "#5A7386"],
  ["Waiting", "#2F8FCF"],
  ["In progress", "#D9902F"],
  ["Done", C.ok],
  ["Archive", "#8FA3B5"],
];

/** The app window: traffic lights, glass sidebar, and a main pane. */
export const AppWindow: React.FC<{ active?: string; counts?: Record<string, number>; children: React.ReactNode; style?: React.CSSProperties }> = ({
  active = "Unread",
  counts = {},
  children,
  style,
}) => (
  <Glass radius={30} style={{ display: "flex", overflow: "hidden", padding: 0, ...style }}>
    <div style={{ width: 330, padding: "26px 18px", display: useStage().portrait ? "none" : "flex", flexDirection: "column", gap: 6, background: "rgba(255,255,255,0.35)" }}>
      <div style={{ display: "flex", gap: 10, marginBottom: 22, paddingLeft: 8 }}>
        {["#FF5F57", "#FEBC2E", "#28C840"].map((c) => (
          <span key={c} style={{ width: 15, height: 15, borderRadius: 8, background: c }} />
        ))}
      </div>
      <div style={{ fontSize: 15, fontWeight: 700, letterSpacing: 1.2, color: C.text3, padding: "0 12px 8px" }}>WORKFLOW</div>
      {STATUSES.map(([s, color]) => (
        <div
          key={s}
          style={{
            display: "flex",
            alignItems: "center",
            gap: 14,
            height: 46,
            padding: "0 14px",
            borderRadius: 12,
            fontSize: 21,
            fontWeight: s === active ? 650 : 450,
            color: C.text,
            background: s === active ? "rgba(31,111,181,0.13)" : "transparent",
          }}
        >
          <span style={{ width: 16, height: 16, borderRadius: 8, border: `2.5px solid ${color}`, background: s === "Unread" || s === "Done" ? color : "transparent" }} />
          {s}
          <span style={{ flex: 1 }} />
          {counts[s] !== undefined && counts[s] > 0 && <span style={{ fontSize: 18, color: C.text3 }}>{counts[s]}</span>}
        </div>
      ))}
    </div>
    <div style={{ flex: 1, background: "rgba(255,255,255,0.72)", position: "relative", overflow: "hidden" }}>{children}</div>
  </Glass>
);

export const Keycap: React.FC<{ label: string; pressed?: number }> = ({ label, pressed = 0 }) => (
  <div
    style={{
      minWidth: 96,
      height: 96,
      padding: "0 22px",
      borderRadius: 22,
      display: "flex",
      alignItems: "center",
      justifyContent: "center",
      fontSize: 44,
      fontWeight: 600,
      color: C.text,
      background: "linear-gradient(180deg, #FFFFFF, #F1F6FB)",
      border: "1.5px solid rgba(255,255,255,0.95)",
      boxShadow: `0 ${10 - 7 * pressed}px ${26 - 16 * pressed}px rgba(31,111,181,${0.18 + 0.1 * pressed}), inset 0 -${4 - 3 * pressed}px 0 #DCE7F2`,
      transform: `translateY(${6 * pressed}px)`,
    }}
  >
    {label}
  </div>
);

/** Big headline above the product shot. */
export const Headline: React.FC<{ children: React.ReactNode; sub?: string; delay?: number; size?: number }> = ({ children, sub, delay = 0, size }) => {
  const p = useIn(delay, 18);
  size = size ?? (useStage().portrait ? 92 : 76);
  return (
    <div style={{ textAlign: "center", opacity: p, transform: `translateY(${(1 - p) * 30}px)` }}>
      <div style={{ fontSize: size, fontWeight: 700, letterSpacing: -2, color: C.text, lineHeight: 1.05 }}>{children}</div>
      {sub && <div style={{ fontSize: size * 0.36, color: C.text2, marginTop: 18, fontWeight: 450 }}>{sub}</div>}
    </div>
  );
};
