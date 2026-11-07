/// <reference lib="webworker" />

// Solves a proof-of-work challenge off the page's thread (`proof-of-work.ts`). It imports only
// the solver, without Angular, to stay small.

import { type Challenge, solve } from './proof-of-work-solver';

addEventListener('message', async ({ data }: MessageEvent<Challenge>) => {
  postMessage(await solve(data));
});
