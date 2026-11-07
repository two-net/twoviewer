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
import { Router, RouterLink } from '@angular/router';
import { firstValueFrom } from 'rxjs';
import { PRODUCT_NAME } from '../product';
import { ErrorMessage } from '../shared/errors/error-message';
import { tryAgainAfter } from '../shared/errors/retry-after';
import { UserError } from '../shared/errors/user-error';
import { USER_LOCALE } from '../shared/format/user-locale';

type Field = 'email' | 'password' | 'code';

/** Why the server didn't accept a field (`server/src/api.rs`). */
interface FieldError {
  readonly code: string;
}

/** Where a successful sign-in leads. */
const HOME = '/devices';

/**
 * Sign-in with an email address and a password (AUTH-05).
 *
 * The server answers a wrong password and an address without an account alike (SEC-14), so the
 * page never says which of the two was wrong. After too many failed attempts with an address,
 * sign-in with it is blocked for a while (AUTH-08), and the page says until when.
 *
 * With MFA on, the right password leads to a second step, which asks for a code from the
 * authenticator app, or a recovery code instead. The server keeps the pending sign-in in a cookie
 * that page scripts can't read, for a few minutes, after which the page asks for the password
 * again.
 */
@Component({
  selector: 'app-sign-in',
  imports: [ReactiveFormsModule, RouterLink, ErrorMessage],
  templateUrl: './sign-in.html',
  styleUrl: './account-form.scss',
})
export class SignIn {
  private readonly http = inject(HttpClient);
  private readonly router = inject(Router);
  private readonly locale = inject(USER_LOCALE);
  private readonly injector = inject(Injector);
  private readonly formElement = viewChild<ElementRef<HTMLFormElement>>('formElement');

  protected readonly productName = PRODUCT_NAME;

  // No length rule for the password: passwords set under earlier rules still work.
  protected readonly form = inject(NonNullableFormBuilder).group({
    email: ['', [Validators.required, Validators.email]],
    password: ['', Validators.required],
  });
  protected readonly codeForm = inject(NonNullableFormBuilder).group({
    code: ['', Validators.required],
  });

  /** The password, then, with MFA on, the code. */
  protected readonly step = signal<'password' | 'code'>('password');
  /** Whether the second step takes a recovery code rather than one from the app. */
  protected readonly useRecovery = signal(false);

  /** Whether the form was submitted, after which every field shows its errors. */
  private readonly submitted = signal(false);
  /** Why the server didn't accept fields, until they change. */
  private readonly serverErrors = signal<Partial<Record<Field, FieldError>>>({});
  protected readonly busy = signal(false);
  protected readonly error = signal<unknown>(null);

  constructor() {
    for (const field of ['email', 'password'] as const) {
      this.form.controls[field].valueChanges.subscribe(() =>
        this.serverErrors.update(({ [field]: _, ...others }) => others),
      );
    }
    this.codeForm.controls.code.valueChanges.subscribe(() =>
      this.serverErrors.update(({ code: _, ...others }) => others),
    );
  }

  /** The error to show for `field`, if any. */
  protected errorFor(field: Field): string | null {
    const control = field === 'code' ? this.codeForm.controls.code : this.form.controls[field];
    const server = this.serverErrors()[field];
    if (server) {
      return this.describe(field, server.code);
    }
    if (!control.invalid || !(control.touched || this.submitted())) {
      return null;
    }
    return this.describe(field, control.errors?.['required'] ? 'required' : 'invalid');
  }

  private describe(field: Field, code: string): string {
    switch (`${field}.${code}`) {
      case 'email.required':
        return $localize`Enter your email address.`;
      case 'email.invalid':
        return $localize`Enter an email address such as name@example.com.`;
      case 'password.required':
        return $localize`Enter your password.`;
      case 'code.required':
        return this.useRecovery()
          ? $localize`Enter one of your recovery codes.`
          : $localize`Enter the code that your authenticator app shows.`;
      default:
        return $localize`Check this field.`;
    }
  }

  protected async submit(): Promise<void> {
    if (this.busy()) {
      return;
    }
    this.submitted.set(true);
    this.error.set(null);
    if (this.form.invalid) {
      afterNextRender(() => this.focusFirstError(), { injector: this.injector });
      return;
    }
    this.busy.set(true);
    try {
      const { email, password } = this.form.getRawValue();
      await firstValueFrom(this.http.post('/api/sign-in', { email, password }));
      // The server has set the sign-in cookie.
      await this.router.navigateByUrl(HOME);
    } catch (error) {
      this.failed(error);
    } finally {
      this.busy.set(false);
    }
  }

  /** Sends the code of the second step. */
  protected async submitCode(): Promise<void> {
    if (this.busy()) {
      return;
    }
    this.submitted.set(true);
    this.error.set(null);
    if (this.codeForm.invalid) {
      afterNextRender(() => this.focusFirstError(), { injector: this.injector });
      return;
    }
    this.busy.set(true);
    try {
      const { code } = this.codeForm.getRawValue();
      await firstValueFrom(this.http.post('/api/sign-in/second-factor', { code }));
      // The server has set the sign-in cookie.
      await this.router.navigateByUrl(HOME);
    } catch (error) {
      this.codeFailed(error);
    } finally {
      this.busy.set(false);
    }
  }

  /** Switches between a code from the app and a recovery code. */
  protected toggleRecovery(): void {
    this.useRecovery.update((useRecovery) => !useRecovery);
    this.codeForm.reset();
    this.submitted.set(false);
    this.error.set(null);
    afterNextRender(() => this.focus('sign-in-code'), { injector: this.injector });
  }

  /** Goes back to the password, to sign in again, perhaps with another account. */
  protected startOver(): void {
    this.toPasswordStep();
    this.error.set(null);
  }

  private toCodeStep(): void {
    this.step.set('code');
    this.useRecovery.set(false);
    this.codeForm.reset();
    this.submitted.set(false);
    afterNextRender(() => this.focus('sign-in-code'), { injector: this.injector });
  }

  private toPasswordStep(): void {
    this.step.set('password');
    this.form.controls.password.reset();
    this.submitted.set(false);
    afterNextRender(() => this.focus('sign-in-password'), { injector: this.injector });
  }

  private failed(error: unknown): void {
    const response = error instanceof HttpErrorResponse ? error : undefined;
    switch (response?.status) {
      case 428:
        if (response.error?.error === 'second_factor_required') {
          this.toCodeStep();
        } else {
          this.error.set(error);
        }
        return;
      case 401:
        this.error.set({
          summary: $localize`The email address or password isn't right.`,
          action: $localize`Check both and try again. If you've forgotten your password, reset it.`,
        } satisfies UserError);
        return;
      case 422:
        this.serverErrors.set(response.error?.fields ?? {});
        afterNextRender(() => this.focusFirstError(), { injector: this.injector });
        return;
      case 429:
        this.error.set(
          tryAgainAfter(
            response,
            $localize`There have been too many attempts to sign in.`,
            this.locale,
          ),
        );
        return;
      default:
        this.error.set(error);
    }
  }

  private codeFailed(error: unknown): void {
    const response = error instanceof HttpErrorResponse ? error : undefined;
    switch (response?.status) {
      case 401:
        this.error.set(
          this.useRecovery()
            ? ({
                summary: $localize`That recovery code isn't right, or has been used.`,
                action: $localize`Check it and try again. Each recovery code works once.`,
              } satisfies UserError)
            : ({
                summary: $localize`That code isn't right.`,
                action: $localize`Enter the code that your authenticator app shows now. Each code works once.`,
              } satisfies UserError),
        );
        this.codeForm.reset();
        this.submitted.set(false);
        afterNextRender(() => this.focus('sign-in-code'), { injector: this.injector });
        return;
      case 410:
        // The pending sign-in has expired.
        this.toPasswordStep();
        this.error.set({
          summary: $localize`Signing in took too long.`,
          action: $localize`Enter your password again.`,
        } satisfies UserError);
        return;
      case 422:
        this.serverErrors.set(response.error?.fields ?? {});
        afterNextRender(() => this.focusFirstError(), { injector: this.injector });
        return;
      case 429:
        this.error.set(
          tryAgainAfter(response, $localize`There have been too many wrong codes.`, this.locale),
        );
        return;
      default:
        this.error.set(error);
    }
  }

  private focus(id: string): void {
    this.formElement()?.nativeElement.querySelector<HTMLElement>(`#${id}`)?.focus();
  }

  private focusFirstError(): void {
    this.formElement()?.nativeElement.querySelector<HTMLElement>('[aria-invalid="true"]')?.focus();
  }
}
