import { DOCUMENT } from '@angular/common';
import { inject } from '@angular/core';
import { ViewTransitionInfo } from '@angular/router';
import { REDUCED_MOTION } from './reduced-motion';

/** On `<html>` during the cross-fade, which `styles.scss` times (ANI-02). */
const CHANGING_CLASS = 'screen-change';

/** The running cross-fade, if any. */
let running: ViewTransition | undefined;

/**
 * Cross-fades the page when the screen changes (ANI-01). The router starts a view transition for
 * every navigation, and calls this in an injection context once it has.
 *
 * With reduced motion it leaves the page without the class, and `styles.scss` gives a view
 * transition without one no animation (ANI-04). Skipping the transition instead would make the
 * router report an error in development builds.
 */
export function screenTransition({ transition }: ViewTransitionInfo): void {
  if (inject(REDUCED_MOTION)()) {
    return;
  }
  const root = inject(DOCUMENT).documentElement;
  root.classList.add(CHANGING_CLASS);
  running = transition;
  // A newer change skips this cross-fade, and the class stays for that one.
  const done = () => {
    if (running === transition) {
      running = undefined;
      root.classList.remove(CHANGING_CLASS);
    }
  };
  transition.finished.then(done, done);
}
