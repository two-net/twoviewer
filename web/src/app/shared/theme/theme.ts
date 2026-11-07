import { DOCUMENT } from '@angular/common';
import { DestroyRef, Injectable, inject, signal } from '@angular/core';
import { REDUCED_MOTION } from '../motion/reduced-motion';

/** The appearance options (THM-01). System follows the operating system or browser (THM-02). */
export type ThemeOption = 'light' | 'dark' | 'system';

export const THEME_OPTIONS: readonly ThemeOption[] = ['light', 'dark', 'system'];

/**
 * Where this browser remembers the option. `public/appearance.js` reads it before the first paint,
 * so the two must agree.
 */
export const THEME_STORAGE_KEY = 'twoviewer.theme';

/** On `<html>` for Light and Dark. `styles.scss` sets `color-scheme` from it. */
const THEME_ATTRIBUTE = 'data-theme';

/** On `<html>` during the cross-fade, which `styles.scss` makes short (THM-03, at most 200 ms). */
const CHANGING_CLASS = 'theme-change';

function toOption(value: string | null | undefined): ThemeOption {
  return THEME_OPTIONS.find((option) => option === value) ?? 'system';
}

/**
 * The theme option: Light, Dark or System, which is the default (THM-01).
 *
 * It is remembered in this browser, so that the pages before sign-in use the last option chosen
 * here (THM-04). `public/appearance.js` applies it before the first paint (THM-05), and this
 * service from then on. Other tabs follow a change.
 *
 * The colors themselves come from the tokens in `styles.scss`. With System, the browser switches
 * them when the system's preference changes, without any code here (THM-02).
 */
@Injectable({ providedIn: 'root' })
export class Theme {
  private readonly document = inject(DOCUMENT);
  private readonly reducedMotion = inject(REDUCED_MOTION);

  private readonly current = signal<ThemeOption>(toOption(this.recall()));
  readonly option = this.current.asReadonly();

  /** The running cross-fade, if any. */
  private transition?: ViewTransition;

  private readonly onStorage = (event: StorageEvent) => {
    if (event.key === THEME_STORAGE_KEY) {
      this.apply(toOption(event.newValue));
    }
  };

  constructor() {
    // `appearance.js` has done this already, unless it could not run.
    this.apply(this.current());
    const window = this.document.defaultView;
    window?.addEventListener('storage', this.onStorage);
    inject(DestroyRef).onDestroy(() => window?.removeEventListener('storage', this.onStorage));
  }

  /**
   * Applies the option that the user chose, at once and with a cross-fade (THM-03), and remembers
   * it in this browser (THM-04).
   *
   * Once there are accounts, this is also where the option is saved in the user's preferences.
   * After sign-in, the account's option is applied with {@link apply} and remembered, but not
   * saved back.
   */
  choose(option: ThemeOption): void {
    this.remember(option);
    if (option === this.current()) {
      return;
    }
    const document = this.document;
    if (
      typeof document.startViewTransition !== 'function' ||
      document.visibilityState !== 'visible' ||
      this.reducedMotion()
    ) {
      this.apply(option);
      return;
    }
    const root = document.documentElement;
    root.classList.add(CHANGING_CLASS);
    const transition = document.startViewTransition(() => this.apply(option));
    this.transition = transition;
    // A newer change skips this cross-fade, and the class stays for that one.
    const done = () => {
      if (this.transition === transition) {
        this.transition = undefined;
        root.classList.remove(CHANGING_CLASS);
      }
    };
    transition.finished.then(done, done);
  }

  /** Shows the option, without a cross-fade. */
  private apply(option: ThemeOption): void {
    this.current.set(option);
    const root = this.document.documentElement;
    if (option === 'system') {
      root.removeAttribute(THEME_ATTRIBUTE);
    } else {
      root.setAttribute(THEME_ATTRIBUTE, option);
    }
  }

  // Storage can be unavailable, for example when the user blocks site data. Then the option lasts
  // until the page is closed, and System applies on the next visit.

  private recall(): string | null {
    try {
      return this.document.defaultView?.localStorage.getItem(THEME_STORAGE_KEY) ?? null;
    } catch {
      return null;
    }
  }

  private remember(option: ThemeOption): void {
    try {
      this.document.defaultView?.localStorage.setItem(THEME_STORAGE_KEY, option);
    } catch {
      // As above.
    }
  }
}
