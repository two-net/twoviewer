import { Injectable, signal } from '@angular/core';

/** What a confirmation asks. The texts are already translated. */
export interface ConfirmOptions {
  readonly title: string;
  readonly message: string;
  /** The label of the button that carries out the action, such as "Remove device". */
  readonly confirmLabel: string;
  /**
   * Text the user must type before they can confirm, such as the account's email address when
   * deleting the account (UI-03). Case and surrounding spaces are ignored, as for email
   * addresses (DB-13).
   */
  readonly typeToConfirm?: string;
}

export interface ConfirmRequest extends ConfirmOptions {
  readonly resolve: (confirmed: boolean) => void;
}

/**
 * Asks the user to confirm a destructive action, such as removing a device (UI-03). The
 * `<app-confirm-dialog>` in the root shows the question.
 */
@Injectable({ providedIn: 'root' })
export class Confirmation {
  private readonly open = signal<ConfirmRequest | undefined>(undefined);
  readonly request = this.open.asReadonly();

  /** Resolves to `true` if the user confirms. Any question still open is answered `false`. */
  confirm(options: ConfirmOptions): Promise<boolean> {
    this.answer(false);
    return new Promise((resolve) => this.open.set({ ...options, resolve }));
  }

  /** Answers the open question, if there is one. */
  answer(confirmed: boolean): void {
    const request = this.open();
    if (request) {
      this.open.set(undefined);
      request.resolve(confirmed);
    }
  }
}

/** Whether `typed` matches `expected`, ignoring case and surrounding spaces. */
export function matchesTyped(typed: string, expected: string): boolean {
  return typed.trim().toLowerCase() === expected.trim().toLowerCase();
}
