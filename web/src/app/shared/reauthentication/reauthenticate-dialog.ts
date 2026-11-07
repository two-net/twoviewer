import { HttpErrorResponse } from '@angular/common/http';
import {
  Component,
  ElementRef,
  afterRenderEffect,
  inject,
  linkedSignal,
  signal,
  viewChild,
} from '@angular/core';
import { ErrorMessage } from '../errors/error-message';
import { tryAgainAfter } from '../errors/retry-after';
import { UserError } from '../errors/user-error';
import { USER_LOCALE } from '../format/user-locale';
import { Reauthentication, ReauthenticationRequest } from './reauthentication';

type Field = 'password' | 'code';

/**
 * Asks for the password, and a code if MFA is on, for the open request of
 * {@link Reauthentication}, in a modal dialog (AUTH-10). Unlike a confirmation, it stays open while
 * the server checks them, and says what was wrong, so that the user can try again or cancel.
 */
@Component({
  selector: 'app-reauthenticate-dialog',
  imports: [ErrorMessage],
  templateUrl: './reauthenticate-dialog.html',
  styleUrls: ['../confirm/confirm-dialog.scss', './reauthenticate-dialog.scss'],
})
export class ReauthenticateDialog {
  protected readonly reauthentication = inject(Reauthentication);
  private readonly locale = inject(USER_LOCALE);
  private readonly dialog = viewChild.required<ElementRef<HTMLDialogElement>>('dialog');

  // Empty again for every new request.
  protected readonly password = linkedSignal({
    source: this.reauthentication.request,
    computation: () => '',
  });
  protected readonly code = linkedSignal({
    source: this.reauthentication.request,
    computation: () => '',
  });
  /** The fields left empty at the last attempt. */
  protected readonly missing = linkedSignal<ReauthenticationRequest | undefined, Field[]>({
    source: this.reauthentication.request,
    computation: () => [],
  });
  protected readonly error = linkedSignal<ReauthenticationRequest | undefined, unknown>({
    source: this.reauthentication.request,
    computation: () => null,
  });
  protected readonly busy = signal(false);

  constructor() {
    // After rendering, so that showModal() finds the password field to focus.
    afterRenderEffect(() => {
      const request = this.reauthentication.request();
      const dialog = this.dialog().nativeElement;
      if (request && !dialog.open) {
        dialog.showModal();
      } else if (!request && dialog.open) {
        dialog.close();
      }
    });
  }

  /** The error to show for `field`, if any. */
  protected errorFor(field: Field): string | null {
    if (!this.missing().includes(field)) {
      return null;
    }
    return field === 'password'
      ? $localize`Enter your password.`
      : $localize`Enter a code from your authenticator app, or a recovery code.`;
  }

  protected cancel(): void {
    this.reauthentication.answer(false);
  }

  protected async submit(event: Event, request: ReauthenticationRequest): Promise<void> {
    event.preventDefault();
    if (this.busy()) {
      return;
    }
    const password = this.password();
    const code = request.secondFactor ? this.code().trim() : undefined;
    const missing: Field[] = [];
    if (password === '') {
      missing.push('password');
    }
    if (code === '') {
      missing.push('code');
    }
    this.missing.set(missing);
    this.error.set(null);
    if (missing.length > 0) {
      this.focus(missing[0]);
      return;
    }
    this.busy.set(true);
    try {
      await this.reauthentication.reauthenticate(password, code);
    } catch (error) {
      this.failed(error, request);
    } finally {
      this.busy.set(false);
    }
  }

  private failed(error: unknown, request: ReauthenticationRequest): void {
    const response = error instanceof HttpErrorResponse ? error : undefined;
    if (response?.status === 401 && response.error?.error === 'reauthentication_failed') {
      this.error.set(
        request.secondFactor
          ? ({
              summary: $localize`The password or code isn't right.`,
              action: $localize`Check both and try again. Each code from your app works once.`,
            } satisfies UserError)
          : ({
              summary: $localize`The password isn't right.`,
              action: $localize`Check it and try again.`,
            } satisfies UserError),
      );
      this.password.set('');
      this.code.set('');
      this.focus('password');
    } else if (response?.status === 429) {
      this.error.set(
        tryAgainAfter(response, $localize`There have been too many wrong attempts.`, this.locale),
      );
    } else {
      this.error.set(error);
    }
  }

  private focus(field: Field): void {
    this.dialog()
      .nativeElement.querySelector<HTMLInputElement>(`#reauthenticate-${field}`)
      ?.focus();
  }
}
