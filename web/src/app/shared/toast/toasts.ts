import { Injectable, signal } from '@angular/core';

/**
 * How long a toast stays (UI-08). A constant for now: the web client gets no settings from the
 * server yet, so operators cannot change it (MNT-01).
 */
export const TOAST_DURATION_MS = 5000;

export interface Toast {
  readonly id: number;
  /** Already translated. */
  readonly message: string;
}

/** Why a toast is paused. It runs again only when nothing holds it. */
export type ToastHold = 'hover' | 'focus';

interface Timer {
  remainingMs: number;
  startedAt: number;
  handle?: ReturnType<typeof setTimeout>;
  readonly holds: Set<ToastHold>;
}

/**
 * Confirms completed background actions, such as a paired device or a finished transfer (UI-08).
 * A toast disappears after {@link TOAST_DURATION_MS}, and its time stands still while it is
 * hovered or focused. `<app-toast-region>` in the root shows the toasts.
 */
@Injectable({ providedIn: 'root' })
export class Toasts {
  private readonly shown = signal<readonly Toast[]>([]);
  readonly toasts = this.shown.asReadonly();
  private readonly timers = new Map<number, Timer>();
  private nextId = 1;

  show(message: string): void {
    const toast = { id: this.nextId++, message };
    this.shown.update((toasts) => [...toasts, toast]);
    this.timers.set(toast.id, { remainingMs: TOAST_DURATION_MS, startedAt: 0, holds: new Set() });
    this.run(toast.id);
  }

  dismiss(id: number): void {
    clearTimeout(this.timers.get(id)?.handle);
    this.timers.delete(id);
    this.shown.update((toasts) => toasts.filter((toast) => toast.id !== id));
  }

  hold(id: number, reason: ToastHold): void {
    const timer = this.timers.get(id);
    if (!timer) {
      return;
    }
    if (timer.holds.size === 0) {
      clearTimeout(timer.handle);
      timer.remainingMs -= Date.now() - timer.startedAt;
    }
    timer.holds.add(reason);
  }

  release(id: number, reason: ToastHold): void {
    const timer = this.timers.get(id);
    if (timer?.holds.delete(reason) && timer.holds.size === 0) {
      this.run(id);
    }
  }

  private run(id: number): void {
    const timer = this.timers.get(id)!;
    timer.startedAt = Date.now();
    timer.handle = setTimeout(() => this.dismiss(id), timer.remainingMs);
  }
}
