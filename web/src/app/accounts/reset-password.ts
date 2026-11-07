import { Location } from '@angular/common';
import { HttpClient, HttpErrorResponse } from '@angular/common/http';
import {
  Component,
  ElementRef,
  Injector,
  afterNextRender,
  inject,
  signal,
  viewChild,
} from '@angular/core';
import { NonNullableFormBuilder, ReactiveFormsModule, Validators } from '@angular/forms';
import { ActivatedRoute, RouterLink } from '@angular/router';
import { firstValueFrom } from 'rxjs';
import { ErrorMessage } from '../shared/errors/error-message';
import { tryAgainAfter } from '../shared/errors/retry-after';
import { USER_LOCALE } from '../shared/format/user-locale';
import { MIN_PASSWORD_LENGTH, RESET_LINK_HOURS } from './account-rules';

/** Where the page is: asking for a link, or using the one from the email. */
type Stage =
  /** The form that asks for a link. */
  | 'request'
  /** The link is on its way, if the address has an account. */
  | 'sent'
  /** The link from the email is being checked. */
  | 'checking'
  /** The link works: the form for the new password. */
  | 'ready'
  /** The link has been used or has expired. */
  | 'expired'
  /** The link couldn't be checked. */
  | 'failed'
  /** The password is reset. */
  | 'done';

type Field = 'email' | 'password';

/** Why the server didn't accept a field (`server/src/api.rs`). */
interface FieldError {
  readonly code: string;
  readonly min?: number;
  readonly max?: number;
}

const PAGE = '/reset-password';

/**
 * Password reset (AUTH-07), in two parts on one page.
 *
 * Without a token, it asks for the email address and has the server email a link. The server
 * answers the same whether or not the address is registered (SEC-14), so the page always says to
 * check the email.
 *
 * The link leads back here: `/reset-password#<token>`. As on the email verification page, the
 * token is in the fragment, which browsers never send to a server, and the page takes it out of the
 * address bar. The page checks that the link still works, then asks for the new password. Setting
 * it signs the user out everywhere, and they sign in again with it, with their second factor if MFA
 * is on.
 */
@Component({
  selector: 'app-reset-password',
  imports: [ReactiveFormsModule, RouterLink, ErrorMessage],
  templateUrl: './reset-password.html',
  styleUrl: './account-form.scss',
})
export class ResetPassword {
  private readonly http = inject(HttpClient);
  private readonly locale = inject(USER_LOCALE);
  private readonly injector = inject(Injector);
  private readonly formElement = viewChild<ElementRef<HTMLFormElement>>('formElement');
  private readonly heading = viewChild<ElementRef<HTMLElement>>('heading');
  /** The token from the link, if the page was opened with one. */
  private readonly token: string | null;

  protected readonly minPasswordLength = MIN_PASSWORD_LENGTH;
  protected readonly linkHours = RESET_LINK_HOURS;

  protected readonly requestForm = inject(NonNullableFormBuilder).group({
    email: ['', [Validators.required, Validators.email]],
  });
  protected readonly passwordForm = inject(NonNullableFormBuilder).group({
    password: ['', [Validators.required, Validators.minLength(MIN_PASSWORD_LENGTH)]],
  });

  protected readonly stage = signal<Stage>('request');
  /** Whether the form shown was submitted, after which its field shows its errors. */
  private readonly submitted = signal(false);
  /** Why the server didn't accept fields, until they change. */
  private readonly serverErrors = signal<Partial<Record<Field, FieldError>>>({});
  protected readonly busy = signal(false);
  protected readonly error = signal<unknown>(null);
  /** The address the link was sent to, or the address of the link's account. */
  protected readonly email = signal('');

  constructor() {
    this.token = inject(ActivatedRoute).snapshot.fragment || null;
    if (this.token) {
      // Out of the address bar and the history, where others could find it.
      inject(Location).replaceState(PAGE);
      void this.check();
    }
    // A field's answer from the server applies until the field changes.
    this.requestForm.controls.email.valueChanges.subscribe(() => this.clearServerError('email'));
    this.passwordForm.controls.password.valueChanges.subscribe(() =>
      this.clearServerError('password'),
    );
  }

  /** The error to show for `field`, if any. */
  protected errorFor(field: Field): string | null {
    const control =
      field === 'email' ? this.requestForm.controls.email : this.passwordForm.controls.password;
    const server = this.serverErrors()[field];
    if (server) {
      return this.describe(field, server);
    }
    if (!control.invalid || !(control.touched || this.submitted())) {
      return null;
    }
    const errors = control.errors ?? {};
    if (errors['required']) {
      return this.describe(field, { code: 'required' });
    }
    if (errors['minlength']) {
      return this.describe(field, { code: 'too_short', min: errors['minlength'].requiredLength });
    }
    return this.describe(field, { code: 'invalid' });
  }

  private describe(field: Field, error: FieldError): string {
    switch (`${field}.${error.code}`) {
      case 'email.required':
        return $localize`Enter your email address.`;
      case 'email.invalid':
        return $localize`Enter an email address such as name@example.com.`;
      case 'password.required':
        return $localize`Enter a new password.`;
      case 'password.too_short':
        return $localize`Use at least ${error.min}:min: characters.`;
      case 'password.too_long':
        return $localize`Use at most ${error.max}:max: characters.`;
      case 'password.compromised':
        return $localize`This password is on a list of passwords stolen from other sites. Choose another one.`;
      default:
        return $localize`Check this field.`;
    }
  }

  /** Has the server email a link to the address in the form. */
  protected async requestLink(): Promise<void> {
    if (!this.ready(this.requestForm.invalid)) {
      return;
    }
    const { email } = this.requestForm.getRawValue();
    this.busy.set(true);
    try {
      await firstValueFrom(this.http.post('/api/password-reset', { email }));
      this.email.set(email.trim());
      this.show('sent');
    } catch (error) {
      const response = error instanceof HttpErrorResponse ? error : undefined;
      switch (response?.status) {
        case 422:
          this.rejected(response);
          return;
        case 429:
          this.error.set(
            tryAgainAfter(
              response,
              $localize`Too many password resets have been asked for from your network.`,
              this.locale,
            ),
          );
          return;
        default:
          this.error.set(error);
      }
    } finally {
      this.busy.set(false);
    }
  }

  /** Checks that the link still works, before asking for a new password. */
  protected async check(): Promise<void> {
    this.stage.set('checking');
    this.error.set(null);
    try {
      const link = await firstValueFrom(
        this.http.post<{ email: string }>('/api/password-reset/check', { token: this.token }),
      );
      this.email.set(link.email);
      this.show('ready', 'reset-password-new');
    } catch (error) {
      if (error instanceof HttpErrorResponse && error.status === 410) {
        this.show('expired');
      } else {
        this.error.set(error);
        this.stage.set('failed');
      }
    }
  }

  /** Sets the new password with the link. */
  protected async setPassword(): Promise<void> {
    if (!this.ready(this.passwordForm.invalid)) {
      return;
    }
    const { password } = this.passwordForm.getRawValue();
    this.busy.set(true);
    try {
      await firstValueFrom(
        this.http.post('/api/password-reset/complete', { token: this.token, password }),
      );
      this.passwordForm.reset();
      this.show('done');
    } catch (error) {
      const response = error instanceof HttpErrorResponse ? error : undefined;
      switch (response?.status) {
        case 410:
          this.passwordForm.reset();
          this.show('expired');
          return;
        case 422:
          this.rejected(response);
          return;
        default:
          this.error.set(error);
      }
    } finally {
      this.busy.set(false);
    }
  }

  /** Back to the form that asks for a link, to get a new one. */
  protected startOver(): void {
    this.serverErrors.set({});
    this.error.set(null);
    this.show('request', 'reset-password-email');
  }

  /**
   * Whether a form may be sent: it isn't being sent already, and its fields, whose errors it
   * shows from now on, are `invalid` or not.
   */
  private ready(invalid: boolean): boolean {
    if (this.busy()) {
      return false;
    }
    this.submitted.set(true);
    this.error.set(null);
    if (invalid) {
      afterNextRender(() => this.focusFirstError(), { injector: this.injector });
      return false;
    }
    return true;
  }

  /** Shows the fields the server didn't accept. */
  private rejected(response: HttpErrorResponse): void {
    this.serverErrors.set(response.error?.fields ?? {});
    afterNextRender(() => this.focusFirstError(), { injector: this.injector });
  }

  private clearServerError(field: Field): void {
    this.serverErrors.update(({ [field]: _, ...others }) => others);
  }

  /** Goes to `stage`, and moves the focus to the field `focus`, or else to the heading. */
  private show(stage: Stage, focus?: string): void {
    this.submitted.set(false);
    this.stage.set(stage);
    afterNextRender(
      () => {
        const target = focus
          ? this.formElement()?.nativeElement.querySelector<HTMLElement>(`#${focus}`)
          : this.heading()?.nativeElement;
        target?.focus();
      },
      { injector: this.injector },
    );
  }

  private focusFirstError(): void {
    this.formElement()?.nativeElement.querySelector<HTMLElement>('[aria-invalid="true"]')?.focus();
  }
}
