import { Injectable } from '@angular/core';
import { Challenge, Solution, solve } from './proof-of-work-solver';

export type { Challenge, Solution } from './proof-of-work-solver';

/**
 * Solves challenges in a web worker, so that the page stays responsive meanwhile (ANI-02). Specs
 * replace it, since there are no workers there.
 */
@Injectable({ providedIn: 'root' })
export class ProofOfWork {
  solve(challenge: Challenge): Promise<Solution> {
    if (typeof Worker === 'undefined') {
      return solve(challenge);
    }
    return new Promise((resolve, reject) => {
      // Same-origin, so the Content Security Policy lets it run (SEC-10).
      const worker = new Worker(new URL('./proof-of-work.worker', import.meta.url), {
        type: 'module',
      });
      worker.addEventListener('message', ({ data }: MessageEvent<Solution>) => {
        worker.terminate();
        resolve(data);
      });
      worker.addEventListener('error', (event) => {
        worker.terminate();
        reject(event);
      });
      worker.postMessage(challenge);
    });
  }
}
