import { HttpClient, HttpErrorResponse } from '@angular/common/http';
import { Component, ElementRef, Injector, afterNextRender, inject, signal } from '@angular/core';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import { Subject, catchError, filter, map, merge, of, startWith, switchMap } from 'rxjs';
import { PRODUCT_NAME } from '../product';
import { Confirmation } from '../shared/confirm/confirmation';
import { ServerConnection } from '../shared/connection/server-connection';
import { ErrorMessage } from '../shared/errors/error-message';
import { LocaleDatePipe } from '../shared/format/locale-date-pipe';
import { CANCELLED, Reauthentication } from '../shared/reauthentication/reauthentication';
import { Skeleton } from '../shared/skeleton/skeleton';
import { Toasts } from '../shared/toast/toasts';

/** Whether MFA is on, as `GET /api/mfa` says (`server/src/accounts/mfa/settings.rs`). */
export interface MfaStatus {
  readonly enabled: boolean;
  readonly enabledAt?: string;
  readonly recoveryCodesLeft: number;
}

/** A new authenticator app being set up, as `POST /api/mfa/setup` answers. */
export interface MfaSetup {
  /** The secret in base32, for typing into the app. */
  readonly secret: string;
  /** The `otpauth://` URI that the QR code holds. */
  readonly uri: string;
  /** The QR code of the URI: an SVG path on a square of `size` units. */
  readonly qrCode: { readonly size: number; readonly path: string };
}

/** The real-time event that says MFA was turned on or off. */
export const MFA_CHANGED = 'mfa.changed';

/** What is being done, which disables the other buttons. */
type Busy = 'setup' | 'confirm' | 'disable';

/**
 * Two-factor authentication with an authenticator app, in Settings → Security (AUTH-06).
 *
 * Turning it on shows a QR code and the setup key for the app, then takes a code from the app to
 * confirm that it works, which turns MFA on and issues 10 recovery codes. They are shown once, here,
 * to copy or download. Turning it on or off needs a recent authentication (AUTH-10), which
 * {@link Reauthentication} asks for when the server wants it.
 */
@Component({
  selector: 'app-mfa-settings',
  imports: [ErrorMessage, LocaleDatePipe, Skeleton],
  templateUrl: './mfa-settings.html',
  styleUrl: './mfa-settings.scss',
})
export class MfaSettings {
  private readonly http = inject(HttpClient);
  private readonly reauthentication = inject(Reauthentication);
  private readonly confirmation = inject(Confirmation);
  private readonly toasts = inject(Toasts);
  private readonly injector = inject(Injector);
  private readonly host = inject<ElementRef<HTMLElement>>(ElementRef);

  /** Whether MFA is on, once loaded. */
  protected readonly status = signal<MfaStatus | undefined>(undefined);
  /** Why loading the status failed, until it loads. */
  protected readonly loadError = signal<unknown>(null);
  /** The app being set up, until it is confirmed or the user cancels. */
  protected readonly setup = signal<MfaSetup | undefined>(undefined);
  /** The recovery codes just issued, until the user has saved them. Never stored anywhere. */
  protected readonly recoveryCodes = signal<readonly string[] | undefined>(undefined);
  /** The code typed to confirm the app. */
  protected readonly code = signal('');
  /** Why the code didn't confirm the app, until it changes. */
  protected readonly codeError = signal<string | null>(null);
  protected readonly busy = signal<Busy | null>(null);
  /** Why the last action failed, until the next. */
  protected readonly error = signal<unknown>(null);

  protected readonly qrLabel = $localize`QR code for your authenticator app`;

  private readonly reloads = new Subject<void>();

  constructor() {
    const connection = inject(ServerConnection);
    // The latest answer wins: a reload cancels the one before.
    merge(
      this.reloads,
      connection.resyncs,
      connection.events.pipe(filter((event) => event.name === MFA_CHANGED)),
    )
      .pipe(
        startWith(undefined),
        switchMap(() =>
          this.http.get<MfaStatus>('/api/mfa').pipe(
            map((status) => ({ status, error: null })),
            catchError((error: unknown) => of({ status: undefined, error })),
          ),
        ),
        takeUntilDestroyed(),
      )
      .subscribe(({ status, error }) => {
        if (status) {
          this.status.set(status);
        }
        this.loadError.set(error);
      });
  }

  /** The setup key in groups of four, which is easier to type. */
  protected grouped(secret: string): string {
    return secret.match(/.{1,4}/g)?.join(' ') ?? secret;
  }

  protected typeCode(code: string): void {
    this.code.set(code);
    this.codeError.set(null);
  }

  /** Starts setting up an authenticator app. */
  protected async startSetup(): Promise<void> {
    this.busy.set('setup');
    this.error.set(null);
    try {
      const setup = await this.reauthentication.run(() =>
        this.http.post<MfaSetup>('/api/mfa/setup', null),
      );
      if (setup !== CANCELLED) {
        this.code.set('');
        this.codeError.set(null);
        this.setup.set(setup);
        this.focusAfterRender('#mfa-setup-title');
      }
    } catch (error) {
      this.error.set(error);
    } finally {
      this.busy.set(null);
    }
  }

  protected cancelSetup(): void {
    this.setup.set(undefined);
    this.error.set(null);
    this.focusAfterRender('#settings-mfa');
  }

  /** Confirms the app with the code it shows, which turns MFA on. */
  protected async confirm(event: Event): Promise<void> {
    event.preventDefault();
    if (this.busy()) {
      return;
    }
    const code = this.code().trim();
    if (code === '') {
      this.codeError.set($localize`Enter the code that your authenticator app shows.`);
      this.focusAfterRender('#mfa-code');
      return;
    }
    this.busy.set('confirm');
    this.error.set(null);
    try {
      const answer = await this.reauthentication.run(() =>
        this.http.post<{ recoveryCodes: string[] }>('/api/mfa/confirm', { code }),
      );
      if (answer !== CANCELLED) {
        this.setup.set(undefined);
        this.recoveryCodes.set(answer.recoveryCodes);
        this.reloads.next();
        this.focusAfterRender('#mfa-recovery-title');
      }
    } catch (error) {
      if (error instanceof HttpErrorResponse && error.status === 422) {
        this.codeError.set(
          $localize`That code isn't right. Enter the code that your authenticator app shows now.`,
        );
        this.focusAfterRender('#mfa-code');
      } else if (error instanceof HttpErrorResponse && [409, 410].includes(error.status)) {
        // Turned on or off elsewhere meanwhile.
        this.setup.set(undefined);
        this.reloads.next();
        this.error.set({
          summary: $localize`Two-factor authentication was changed in another window.`,
          action: $localize`Check whether it is on, and start again if you need to.`,
        });
      } else {
        this.error.set(error);
      }
    } finally {
      this.busy.set(null);
    }
  }

  /** Copies the recovery codes, one per line. */
  protected async copy(codes: readonly string[]): Promise<void> {
    try {
      await navigator.clipboard.writeText(codes.join('\n'));
      this.toasts.show($localize`Recovery codes copied.`);
    } catch {
      this.toasts.show($localize`Couldn't copy the recovery codes. Select and copy them instead.`);
    }
  }

  /** Saves the recovery codes as a text file. */
  protected download(codes: readonly string[]): void {
    const text = [
      $localize`Recovery codes for ${PRODUCT_NAME}:product::`,
      $localize`Each code works once, instead of a code from your authenticator app.`,
      '',
      ...codes,
      '',
    ].join('\n');
    const url = URL.createObjectURL(new Blob([text], { type: 'text/plain' }));
    const link = document.createElement('a');
    link.href = url;
    link.download = `${PRODUCT_NAME.toLowerCase()}-recovery-codes.txt`;
    link.click();
    URL.revokeObjectURL(url);
  }

  /** Forgets the recovery codes, once the user has saved them. */
  protected done(): void {
    this.recoveryCodes.set(undefined);
    this.toasts.show($localize`Two-factor authentication is on.`);
    this.focusAfterRender('#settings-mfa');
  }

  /** Turns MFA off, once the user confirms. */
  protected async disable(): Promise<void> {
    const confirmed = await this.confirmation.confirm({
      title: $localize`Turn off two-factor authentication?`,
      message: $localize`Signing in will need only your password, and your recovery codes will stop working.`,
      confirmLabel: $localize`Turn off`,
    });
    if (!confirmed) {
      return;
    }
    this.busy.set('disable');
    this.error.set(null);
    try {
      const answer = await this.reauthentication.run(() => this.http.delete('/api/mfa'));
      if (answer !== CANCELLED) {
        this.toasts.show($localize`Two-factor authentication is off.`);
        this.reloads.next();
      }
    } catch (error) {
      this.error.set(error);
    } finally {
      this.busy.set(null);
    }
  }

  private focusAfterRender(selector: string): void {
    afterNextRender(() => this.host.nativeElement.querySelector<HTMLElement>(selector)?.focus(), {
      injector: this.injector,
    });
  }
}
