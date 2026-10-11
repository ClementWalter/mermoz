/**
 * The launch video's scenes. Each scene is a headline plus a product shot drawn on a
 * fixed-size stage; `Shot` scales that stage to fit landscape or portrait output.
 */
import React from "react";
import { AbsoluteFill, Img, interpolate, staticFile, useCurrentFrame, useVideoConfig } from "remotion";
import { AppWindow, C, Chip, Dot, FONT, Glass, Headline, Keycap, MAILS, Row, StageContext, useIn, useStage } from "./ui";

const clamp = { extrapolateLeft: "clamp", extrapolateRight: "clamp" } as const;

/** Headline on top, the stage scaled into the remaining space. The stage is w×h in
 * 16:9 and pw×ph (taller) in 9:16; the headline and scenes read it with `useStage`. */
const Shot: React.FC<{ headline: React.ReactNode; w: number; h: number; pw?: number; ph?: number; children: React.ReactNode }> = (props) => {
  const { headline, children } = props;
  const { width, height } = useVideoConfig();
  const portrait = height > width;
  const w = portrait ? (props.pw ?? 1000) : props.w;
  const h = portrait ? (props.ph ?? 1300) : props.h;
  const top = portrait ? 440 : 230;
  const scale = Math.min((width * (portrait ? 0.94 : 0.86)) / w, (height - top - (portrait ? 160 : 60)) / h);
  return (
    <StageContext.Provider value={{ w, h, portrait }}>
    <AbsoluteFill style={{ fontFamily: FONT }}>
      <div style={{ position: "absolute", top: portrait ? 150 : 70, left: 0, right: 0, padding: "0 60px" }}>{headline}</div>
      <div
        style={{
          position: "absolute",
          top: top + ((height - top - (portrait ? 160 : 60)) - h * scale) / 2,
          left: (width - w * scale) / 2,
          width: w,
          height: h,
          transform: `scale(${scale})`,
          transformOrigin: "top left",
        }}
      >
        {children}
      </div>
    </AbsoluteFill>
    </StageContext.Provider>
  );
};

export const Intro: React.FC = () => {
  const f = useCurrentFrame();
  const { width, height } = useVideoConfig();
  const icon = useIn(4, 11);
  const word = useIn(18, 18);
  const tag = useIn(32, 18);
  const glow = interpolate(f, [0, 90], [0.6, 1], clamp);
  const s = Math.min(width, height) / 1080;
  return (
    <AbsoluteFill style={{ fontFamily: FONT, alignItems: "center", justifyContent: "center" }}>
      <div style={{ display: "flex", flexDirection: "column", alignItems: "center", transform: `scale(${s})` }}>
        <div style={{ position: "relative" }}>
          <div style={{ position: "absolute", inset: -60, borderRadius: "50%", background: "#BFDBF7", filter: "blur(60px)", opacity: 0.7 * glow * icon }} />
          <Img src={staticFile("icon.png")} style={{ width: 260, height: 260, transform: `scale(${0.4 + 0.6 * icon}) rotate(${(1 - icon) * -12}deg)`, opacity: icon, position: "relative" }} />
        </div>
        <div style={{ fontSize: 150, fontWeight: 750, letterSpacing: -5, color: C.text, marginTop: 30, opacity: word, transform: `translateY(${(1 - word) * 30}px)` }}>Mermoz</div>
        <div style={{ fontSize: 50, color: C.text2, marginTop: 6, opacity: tag, transform: `translateY(${(1 - tag) * 20}px)` }}>Email, finally light.</div>
      </div>
    </AbsoluteFill>
  );
};

/** New mail lands, then the AI's suggestion pops on every row. */
export const Sorting: React.FC = () => {
  const f = useCurrentFrame();
  const sweep = interpolate(f, [95, 125], [0, 1], clamp);
  const timer = interpolate(f, [95, 103], [0, 0.25], clamp);
  return (
    <Shot w={1600} h={880} headline={<Headline sub="Every new email gets a status and a project before you open it.">Sorted before you read it.</Headline>}>
      <SortingWindow f={f} sweep={sweep} timer={timer} />
    </Shot>
  );
};

const SortingWindow: React.FC<{ f: number; sweep: number; timer: number }> = ({ f, sweep, timer }) => {
  const { w, h } = useStage();
  return (
      <AppWindow counts={{ Unread: Math.min(6, Math.floor(f / 12)) }} style={{ width: w, height: h }}>
        <div style={{ padding: "26px 30px 18px", display: "flex", alignItems: "center", gap: 14 }}>
          <span style={{ fontSize: 30, fontWeight: 700, color: C.text }}>Unread</span>
          <span style={{ flex: 1 }} />
          <span style={{ opacity: interpolate(f, [92, 100], [0, 1], clamp) }}>
            <Chip ai>sorted in {timer.toFixed(2)} s</Chip>
          </span>
        </div>
        {MAILS.map((m, i) => {
          const land = interpolate(f, [8 + i * 12, 22 + i * 12], [0, 1], clamp);
          const chip = interpolate(sweep * 8 - i * 0.9, [0, 1], [0, 1], clamp);
          return (
            <div key={m.n} style={{ opacity: land, transform: `translateY(${(1 - land) * -40}px)` }}>
              <Row mail={m} chip={chip} />
            </div>
          );
        })}
        <div
          style={{
            position: "absolute",
            top: 0,
            bottom: 0,
            width: 260,
            left: `${-20 + sweep * 120}%`,
            background: "linear-gradient(90deg, transparent, rgba(220,235,250,0.85), transparent)",
            opacity: sweep > 0 && sweep < 1 ? 1 : 0,
          }}
        />
      </AppWindow>
  );
};

/** Keycaps clear the inbox one thread at a time. */
export const Keys: React.FC = () => {
  const f = useCurrentFrame();
  const keys = ["Y", "E", "A", "Y", "⌫", "A"];
  const step = 22;
  const start = 24;
  const pressedIndex = Math.floor((f - start) / step);
  const gone = (i: number) => interpolate(f, [start + i * step + 6, start + i * step + 16], [0, 1], clamp);
  const remaining = MAILS.filter((_, i) => gone(i) < 1).length;
  return (
    <Shot w={1600} h={880} headline={<Headline sub="Accept, done, archive, delete: no mouse, no folders to drag to.">One key. Done.</Headline>}>
      <KeysStage>
        <AppWindow counts={{ Unread: remaining, "To do": MAILS.filter((m, i) => m.sort === "To do" && gone(i) >= 1).length + 4 }} style={{ width: "100%", height: "100%" }}>
          <div style={{ padding: "26px 30px 18px", fontSize: 30, fontWeight: 700, color: C.text }}>Unread</div>
          {MAILS.map((m, i) => {
            const g = gone(i);
            return (
              <div
                key={m.n}
                style={{
                  height: (1 - g) * 128,
                  overflow: "hidden",
                  opacity: 1 - g,
                  transform: `translateX(${-g * 240}px)`,
                }}
              >
                <Row mail={m} chip={1} selected={i === Math.max(0, pressedIndex)} />
              </div>
            );
          })}
          {remaining === 0 && (
            <div style={{ position: "absolute", inset: 0, display: "flex", flexDirection: "column", alignItems: "center", justifyContent: "center", gap: 18, opacity: interpolate(f, [start + 6 * step, start + 6 * step + 12], [0, 1], clamp) }}>
              <div style={{ fontSize: 90, color: C.ok }}>✓</div>
              <div style={{ fontSize: 34, color: C.text2, fontWeight: 500 }}>Inbox zero, in seconds</div>
            </div>
          )}
        </AppWindow>
        <div style={{ position: "absolute", right: 60, bottom: -40, display: "flex", gap: 22, transform: "scale(1.4)", transformOrigin: "right bottom" }}>
          {keys.map((k, i) => {
            const t = f - (start + i * step);
            const visible = interpolate(t, [-6, 0], [0, 1], clamp);
            const pressed = interpolate(t, [0, 4, 10], [0, 1, 0], clamp);
            return i === pressedIndex || (i === pressedIndex + 1 && visible > 0) ? (
              <div key={i} style={{ opacity: i === pressedIndex ? 1 : visible }}>
                <Keycap label={k} pressed={pressed} />
              </div>
            ) : null;
          })}
        </div>
      </KeysStage>
    </Shot>
  );
};

const KeysStage: React.FC<{ children: React.ReactNode }> = ({ children }) => {
  const { w, h } = useStage();
  return <div style={{ position: "relative", width: w, height: h }}>{children}</div>;
};

/** Cards glide across a Linear-style board. */
export const Board: React.FC = () => {
  const f = useCurrentFrame();
  return (
    <Shot w={1600} h={880} ph={1100} headline={<Headline sub="Threads move like tasks. Your Gmail labels are your projects.">Flows, not folders.</Headline>}>
      <BoardStage f={f} />
    </Shot>
  );
};

const BoardStage: React.FC<{ f: number }> = ({ f }) => {
  const { w, h, portrait } = useStage();
  const cols = ["Unread", "To do", "In progress", "Done"];
  const colW = (w - 80 - 3 * 30) / 4;
  const colX = (c: number) => 40 + c * (colW + 30);
  // Each card has its own slot before and after, so no two ever overlap.
  type Card = { title: string; who: string; from: number; to: number; at: number; row: number; toRow: number; color: string; project: string };
  const cards: Card[] = [
    { title: "Q4 audit — sign by Friday", who: "Marc Petit", from: 0, to: 1, at: 30, row: 0, toRow: 0, color: C.orange, project: "Acme" },
    { title: "New mockups attached", who: "Hugo · Design", from: 1, to: 2, at: 60, row: 1, toRow: 1, color: "#C2568C", project: "Website" },
    { title: "Dinner on Saturday?", who: "Léa Martin", from: 0, to: 3, at: 90, row: 1, toRow: 1, color: C.purple, project: "Family" },
    { title: "Contract renewal", who: "Nadia · Legal", from: 2, to: 3, at: 115, row: 0, toRow: 0, color: "#2F8FCF", project: "Acme" },
  ];
  return (
      <Glass radius={30} style={{ width: w, height: h, position: "relative", overflow: "hidden" }}>
        {cols.map((c, i) => (
          <div key={c} style={{ position: "absolute", left: colX(i), top: 36, width: colW, height: h - 80, borderRadius: 22, background: "rgba(255,255,255,0.45)", border: "1.5px solid rgba(255,255,255,0.9)" }}>
            <div style={{ padding: "22px 18px", fontSize: portrait ? 20 : 24, fontWeight: 700, color: C.text, display: "flex", alignItems: "center", gap: 10 }}>
              <span style={{ width: 16, height: 16, borderRadius: 8, border: `2.5px solid ${[C.accent, "#5A7386", "#D9902F", C.ok][i]}`, background: i === 0 || i === 3 ? [C.accent, "", "", C.ok][i] : "transparent" }} />
              {c}
            </div>
          </div>
        ))}
        {cards.map((card) => {
          const p = interpolate(f, [card.at, card.at + 22], [0, 1], { ...clamp, easing: (t) => 1 - Math.pow(1 - t, 3) });
          const x = colX(card.from) + (colX(card.to) - colX(card.from)) * p + 12;
          const lift = Math.sin(p * Math.PI) * 26;
          const y = 110 + (card.row + (card.toRow - card.row) * p) * (portrait ? 230 : 190) - lift;
          return (
            <div key={card.title} style={{ position: "absolute", left: x, top: y, width: colW - 24, transform: `rotate(${Math.sin(p * Math.PI) * 2.5}deg)` }}>
              <div style={{ background: "white", borderRadius: 18, padding: "20px 22px", boxShadow: `0 ${8 + lift}px ${24 + lift}px rgba(31,111,181,${0.12 + lift / 300})`, display: "flex", flexDirection: "column", gap: 12 }}>
                <div style={{ fontSize: 22, fontWeight: 600, color: C.text }}>{card.title}</div>
                <div style={{ display: "flex", flexDirection: portrait ? "column" : "row", alignItems: portrait ? "flex-start" : "center", gap: 10, fontSize: 18, color: C.text2 }}>
                  {card.who}
                  <span style={{ flex: 1 }} />
                  <Chip style={{ height: 28, fontSize: 16 }}>
                    <Dot color={card.color} size={10} /> {card.project}
                  </Chip>
                </div>
              </div>
            </div>
          );
        })}
      </Glass>
  );
};

/** Original → Optimized: the clutter collapses into clean, readable text. */
export const Reading: React.FC = () => {
  const f = useCurrentFrame();
  const t = interpolate(f, [55, 85], [0, 1], { ...clamp, easing: (x) => x * x * (3 - 2 * x) });
  const clutter = (label: string, h: number, color: string) => (
    <div style={{ height: h * (1 - t), opacity: 1 - t, overflow: "hidden", marginBottom: 14 * (1 - t) }}>
      <div style={{ height: h, borderRadius: 12, background: color, display: "flex", alignItems: "center", padding: "0 24px", fontSize: 18, color: C.text3 }}>{label}</div>
    </div>
  );
  return (
    <Shot w={1300} h={880} ph={1250} headline={<Headline sub="Original, as sent. Or Optimized: the same words, without the noise.">Read what matters.</Headline>}>
      <Glass radius={30} style={{ width: "100%", height: "100%", padding: 44, display: "flex", flexDirection: "column", gap: 22 }}>
        <div style={{ display: "flex", alignItems: "center", gap: 18 }}>
          <div style={{ width: 56, height: 56, borderRadius: 28, background: C.orange, color: "white", fontWeight: 700, fontSize: 22, display: "flex", alignItems: "center", justifyContent: "center" }}>MP</div>
          <div>
            <div style={{ fontSize: 25, fontWeight: 650, color: C.text }}>Marc Petit</div>
            <div style={{ fontSize: 19, color: C.text3 }}>to you · 9:41</div>
          </div>
          <span style={{ flex: 1 }} />
          <div style={{ display: "flex", background: "rgba(13,36,54,0.06)", borderRadius: 12, padding: 4, position: "relative" }}>
            <div style={{ position: "absolute", top: 4, bottom: 4, left: 4 + t * 150, width: 146, borderRadius: 9, background: "white", boxShadow: "0 2px 6px rgba(13,36,54,0.12)" }} />
            {["Original", "✦ Optimized"].map((l, i) => (
              <div key={l} style={{ position: "relative", width: 150, height: 44, display: "flex", alignItems: "center", justifyContent: "center", fontSize: 19, fontWeight: 600, color: (i === 1) === t > 0.5 ? C.text : C.text3 }}>
                {l}
              </div>
            ))}
          </div>
        </div>
        <div style={{ fontSize: 34, fontWeight: 700, color: C.text }}>Q4 audit — can you sign by Friday?</div>
        {clutter("[ Acme Corp banner · view in browser ]", 70, "#EEF3F8")}
        <div style={{ fontSize: 24, lineHeight: 1.6, color: C.text }}>
          Hi,
          <br />
          The audit report is ready. Two pages need your signature:
          <div style={{ margin: "10px 0 10px 8px" }}>
            • <b style={{ fontWeight: 600 * t + 400 * (1 - t) }}>page 4</b> — revenue recognition
            <br />• <b style={{ fontWeight: 600 * t + 400 * (1 - t) }}>page 9</b> — related parties
          </div>
          Could you send them back by Friday? Thanks!
          <br />
          Marc
        </div>
        {clutter("Marc Petit | CFO | Acme Corp | +33 6 12 34 56 78 | acme.example", 56, "#F2F6FA")}
        {clutter("This message and any attachments are confidential and intended solely for…", 56, "#F2F6FA")}
        {clutter("> On Tue, Oct 6, you wrote: Sure, send it over when it's ready…", 56, "#F5F2FA")}
      </Glass>
    </Shot>
  );
};

/** ⌘K search across the whole mailbox, offline. */
export const Search: React.FC = () => {
  const f = useCurrentFrame();
  const query = "boarding pass lisbon";
  const typed = query.slice(0, Math.max(0, Math.floor((f - 25) / 2.2)));
  const results = [
    ["Your boarding pass · TP 431", "Lisbon Air · today"],
    ["Hotel confirmation — Alfama", "Booking · 2 days ago"],
    ["Lisbon trip plan", "Léa Martin · last week"],
  ];
  const panel = useIn(6, 16);
  return (
    <Shot w={1300} h={760} ph={900} headline={<Headline sub="Everything stays on your Mac. Search, read and sort offline.">Your whole mailbox. Instant.</Headline>}>
      <div style={{ width: "100%", height: "100%", position: "relative" }}>
        <Glass radius={30} style={{ position: "absolute", inset: 0, opacity: panel, transform: `scale(${0.94 + 0.06 * panel})` }}>
          <div style={{ display: "flex", alignItems: "center", gap: 20, height: 110, padding: "0 40px", borderBottom: `1.5px solid ${C.line}` }}>
            <span style={{ fontSize: 34, color: C.accent }}>⌕</span>
            <span style={{ fontSize: 36, color: typed ? C.text : C.text3 }}>
              {typed || "Search mail or run a command"}
              <span style={{ opacity: Math.floor(f / 8) % 2 ? 1 : 0, color: C.accent }}>|</span>
            </span>
            <span style={{ flex: 1 }} />
            <span style={{ fontSize: 22, color: C.text3, border: `1.5px solid ${C.line}`, borderRadius: 8, padding: "4px 10px", background: "white" }}>⌘K</span>
          </div>
          {results.map(([title, meta], i) => {
            const show = interpolate(f, [70 + i * 5, 80 + i * 5], [0, 1], clamp);
            return (
              <div key={title} style={{ display: "flex", alignItems: "center", gap: 22, height: 100, padding: "0 40px", background: i === 0 ? "rgba(31,111,181,0.10)" : "transparent", opacity: show, transform: `translateY(${(1 - show) * 16}px)` }}>
                <span style={{ width: 16, height: 16, borderRadius: 8, border: `2.5px solid ${i === 0 ? C.accent : "#8FA3B5"}` }} />
                <span style={{ fontSize: 30, color: C.text, fontWeight: 500 }}>{title}</span>
                <span style={{ flex: 1 }} />
                <span style={{ fontSize: 22, color: C.text3 }}>{meta}</span>
              </div>
            );
          })}
          <div style={{ position: "absolute", bottom: 34, left: 40, display: "flex", gap: 16, opacity: interpolate(f, [95, 110], [0, 1], clamp) }}>
            <Chip>Works offline</Chip>
            <Chip>Keyboard first</Chip>
            <Chip>Agents draft · you approve</Chip>
          </div>
        </Glass>
      </div>
    </Shot>
  );
};

export const Outro: React.FC = () => {
  const { width, height } = useVideoConfig();
  const icon = useIn(2, 12);
  const word = useIn(12, 18);
  const tag = useIn(24, 18);
  const cta = useIn(36, 16);
  const s = Math.min(width, height) / 1080;
  return (
    <AbsoluteFill style={{ fontFamily: FONT, alignItems: "center", justifyContent: "center" }}>
      <div style={{ display: "flex", flexDirection: "column", alignItems: "center", transform: `scale(${s})` }}>
        <Img src={staticFile("icon.png")} style={{ width: 200, height: 200, opacity: icon, transform: `scale(${0.6 + 0.4 * icon})` }} />
        <div style={{ fontSize: 120, fontWeight: 750, letterSpacing: -4, color: C.text, marginTop: 26, opacity: word, transform: `translateY(${(1 - word) * 24}px)` }}>Mermoz</div>
        <div style={{ fontSize: 44, color: C.text2, marginTop: 4, opacity: tag }}>Light, fast email for macOS.</div>
        <div style={{ marginTop: 50, opacity: cta, transform: `translateY(${(1 - cta) * 20}px)` }}>
          <div style={{ fontSize: 30, fontWeight: 650, color: "white", background: C.accent, borderRadius: 18, padding: "20px 44px", boxShadow: "0 20px 50px rgba(31,111,181,0.35)" }}>Download for Mac</div>
        </div>
      </div>
    </AbsoluteFill>
  );
};
