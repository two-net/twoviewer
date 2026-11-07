import { DOCUMENT } from '@angular/common';
import { DestroyRef, Injectable, computed, inject, signal } from '@angular/core';

/** The motion options (ANI-05). System follows the operating system or browser. */
export type MotionOption = 'system' | 'full' | 'reduced';

export const MOTION_OPTIONS: readonly MotionOption[] = ['system', 'full', 'reduced'];

/**
 * Where this browser remembers the option. `public/appearance.js` reads it before the first paint,
 * so the two must agree.
 */
export const MOTION_STORAGE_KEY = 'twoviewer.motion';

/** On `<html>` for Full and Reduced. `styles.scss` sets the motion tokens from it. */
const MOTION_ATTRIBUTE = 'data-motion';

const SYSTEM_PREFERENCE = '(prefers-reduced-motion: reduce)';

function toOption(value: string | null | undefined): MotionOption {
  return MOTION_OPTIONS.find((option) => option === value) ?? 'system';
}

/**
 * The motion option: System, which is the default, Full or Reduced (ANI-05).
 *
 * The styles apply it on their own, from `data-motion` and the system's preference (ANI-04). This
 * service sets the attribute, and tells code that starts animations, through `REDUCED_MOTION`,
 * whether motion is reduced right now.
 *
 * Like the theme, it is remembered in this browser, so that the pages before sign-in use the last
 * option chosen here. `public/appearance.js` applies it before the first paint, so that the splash
 * screen follows it (SPL-07), and this service from then on. Other tabs follow a change. Changes are
 * not audited (AUD-13).
 */
@Injectable({ providedIn: 'root' })
export class Motion {
  private readonly document = inject(DOCUMENT);

  private readonly current = signal<MotionOption>(toOption(this.recall()));
  readonly option = this.current.asReadonly();

  /** Whether the operating system or browser asks for less motion. */
  private readonly systemReduces = signal(false);

  /** Whether motion is reduced right now (ANI-04). */
  readonly reduced = computed(() => {
    const option = this.current();
    return option === 'system' ? this.systemReduces() : option === 'reduced';
  });

  private readonly onStorage = (event: StorageEvent) => {
    if (event.key === MOTION_STORAGE_KEY) {
      this.apply(toOption(event.newValue));
    }
  };

  constructor() {
    // `appearance.js` has done this already, unless it could not run.
    this.apply(this.current());
    const window = this.document.defaultView;
    const query = window?.matchMedia?.(SYSTEM_PREFERENCE);
    const onChange = () => this.systemReduces.set(query?.matches ?? false);
    onChange();
    query?.addEventListener('change', onChange);
    window?.addEventListener('storage', this.onStorage);
    inject(DestroyRef).onDestroy(() => {
      query?.removeEventListener('change', onChange);
      window?.removeEventListener('storage', this.onStorage);
    });
  }

  /**
   * Applies the option that the user chose, at once, and remembers it in this browser.
   *
   * Once there are accounts, this is also where the option is saved in the user's preferences
   * (§5.2), as for the theme.
   */
  choose(option: MotionOption): void {
    this.remember(option);
    this.apply(option);
  }

  private apply(option: MotionOption): void {
    this.current.set(option);
    const root = this.document.documentElement;
    if (option === 'system') {
      root.removeAttribute(MOTION_ATTRIBUTE);
    } else {
      root.setAttribute(MOTION_ATTRIBUTE, option);
    }
  }

  // Storage can be unavailable, for example when the user blocks site data. Then the option lasts
  // until the page is closed, and System applies on the next visit.

  private recall(): string | null {
    try {
      return this.document.defaultView?.localStorage.getItem(MOTION_STORAGE_KEY) ?? null;
    } catch {
      return null;
    }
  }

  private remember(option: MotionOption): void {
    try {
      this.document.defaultView?.localStorage.setItem(MOTION_STORAGE_KEY, option);
    } catch {
      // As above.
    }
  }
}
