import { HttpClient, HttpErrorResponse } from '@angular/common/http';
import { Injectable, inject, signal } from '@angular/core';
import { Observable, firstValueFrom } from 'rxjs';

/** What {@link Reauthentication.run} resolves to when the user cancels. */
export const CANCELLED = Symbol('cancelled');

/** An open request to re-authenticate. */
export interface ReauthenticationRequest {
  /** Whether a code is needed as well as the password, because MFA is on. */
  readonly secondFactor: boolean;
  readonly resolve: (reauthenticated: boolean) => void;
}

/**
 * Re-authentication before sensitive changes (AUTH-10): changing the password, email address or MFA
 * settings, turning on unattended access, and deleting the account.
 *
 * The server answers such a change with `428` (`reauthentication_required`) when the password was
 * last entered more than 15 minutes ago. {@link run} then asks for it, and for a code if MFA is on,
 * in the `<app-reauthenticate-dialog>` in the root, and sends the change again once the server has
 * accepted them.
 */
@Injectable({ providedIn: 'root' })
export class Reauthentication {
  private readonly http = inject(HttpClient);
  private readonly open = signal<ReauthenticationRequest | undefined>(undefined);
  readonly request = this.open.asReadonly();

  /**
   * Sends `request()`, re-authenticating first if the server asks for it, and resolves to the
   * answer, or to {@link CANCELLED} if the user cancels. Other errors reject, as they would without
   * this.
   */
  async run<T>(request: () => Observable<T>): Promise<T | typeof CANCELLED> {
    try {
      return await firstValueFrom(request());
    } catch (error) {
      const secondFactor = reauthenticationRequired(error);
      if (secondFactor === undefined) {
        throw error;
      }
      if (!(await this.ask(secondFactor))) {
        return CANCELLED;
      }
      return await firstValueFrom(request());
    }
  }

  /**
   * Asks the user to re-authenticate, and resolves to `true` once the server has accepted them.
   * Any request still open is answered `false`.
   */
  ask(secondFactor: boolean): Promise<boolean> {
    this.answer(false);
    return new Promise((resolve) => this.open.set({ secondFactor, resolve }));
  }

  /**
   * Sends the password, and the code if there is one, for the open request. Resolves once the
   * server accepts them, which answers the request; rejects with the server's answer otherwise.
   */
  async reauthenticate(password: string, code?: string): Promise<void> {
    await firstValueFrom(
      this.http.post('/api/reauthenticate', code === undefined ? { password } : { password, code }),
    );
    this.answer(true);
  }

  /** Answers the open request, if there is one. */
  answer(reauthenticated: boolean): void {
    const request = this.open();
    if (request) {
      this.open.set(undefined);
      request.resolve(reauthenticated);
    }
  }
}

/**
 * If `error` is the server asking to re-authenticate, whether it needs a code too. `undefined`
 * for any other error.
 */
function reauthenticationRequired(error: unknown): boolean | undefined {
  if (
    error instanceof HttpErrorResponse &&
    error.status === 428 &&
    error.error?.error === 'reauthentication_required'
  ) {
    return error.error.secondFactor === true;
  }
  return undefined;
}
