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

type Field = 'email' | 'password';

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
  }

  /** The error to show for `field`, if any. */
  protected errorFor(field: Field): string | null {
    const control = this.form.controls[field];
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

  private failed(error: unknown): void {
    const response = error instanceof HttpErrorResponse ? error : undefined;
    switch (response?.status) {
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

  private focusFirstError(): void {
    this.formElement()?.nativeElement.querySelector<HTMLElement>('[aria-invalid="true"]')?.focus();
  }
}
