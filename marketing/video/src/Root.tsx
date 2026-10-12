/**
 * Composition registry: one timeline, rendered in 16:9 (X, LinkedIn, YouTube) and
 * 9:16 (Reels, TikTok, Shorts).
 */
import React from "react";
import { AbsoluteFill, Composition, Sequence } from "remotion";
import { Agents, Board, Intro, Keys, Outro, Reading, Search, Sorting } from "./scenes";
import { Ground, SceneFade } from "./ui";

const SCENES: [React.FC, number][] = [
  [Intro, 95],
  [Sorting, 190],
  [Keys, 190],
  [Board, 170],
  [Reading, 160],
  [Search, 160],
  [Agents, 200],
  [Outro, 130],
];

// Scenes fade out before the next fades in: two headlines never overlap.
const OVERLAP = 0;
export const TOTAL = SCENES.reduce((n, [, d]) => n + d, 0) - OVERLAP * (SCENES.length - 1);

const Launch: React.FC = () => {
  let at = 0;
  return (
    <AbsoluteFill>
      <Ground />
      {SCENES.map(([Scene, d], i) => {
        const from = at;
        at += d - OVERLAP;
        return (
          <Sequence key={i} from={from} durationInFrames={d}>
            <SceneFade duration={d}>
              <Scene />
            </SceneFade>
          </Sequence>
        );
      })}
    </AbsoluteFill>
  );
};

export const Root: React.FC = () => (
  <>
    <Composition id="Launch" component={Launch} durationInFrames={TOTAL} fps={30} width={1920} height={1080} />
    <Composition id="LaunchVertical" component={Launch} durationInFrames={TOTAL} fps={30} width={1080} height={1920} />
  </>
);
