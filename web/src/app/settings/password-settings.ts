import { HttpClient, HttpErrorResponse } from '@angular/common/http';
import { Component, ElementRef, Injector, afterNextRender, inject, signal } from '@angular/core';
import { NonNullableFormBuilder, ReactiveFormsModule, Validators } from '@angular/forms';
import { MIN_PASSWORD_LENGTH } from '../accounts/account-rules';
import { ErrorMessage } from '../shared/errors/error-message';
import { tryAgainAfter } from '../shared/errors/retry-after';
import { USER_LOCALE } from '../shared/format/user-locale';
import { CANCELLED, Reauthentication } from '../shared/reauthentication/reauthentication';
import { Toasts } from '../shared/toast/toasts';

type Field = 'currentPassword' | 'newPassword';

/** Why the server didn't accept a field (`server/src/api.rs`). */
interface FieldError {
  readonly code: string;
  readonly min?: number;
  readonly max?: number;
}

/**
 * Changing the password, in Settings → Security (AUTH-11). It takes the current password, and a
 * recent authentication too (AUTH-10), which {@link Reauthentication} asks for when the server
 * wants it. Every other browser is signed out; the list of sign-ins follows by itself.
 */
@Component({
  selector: 'app-password-settings',
  imports: [ReactiveFormsModule, ErrorMessage],
  templateUrl: './password-settings.html',
  styleUrls: ['../accounts/account-form.scss', './password-settings.scss'],
})
export class PasswordSettings {
  private readonly http = inject(HttpClient);
  private readonly reauthentication = inject(Reauthentication);
  private readonly toasts = inject(Toasts);
  private readonly locale = inject(USER_LOCALE);
  private readonly injector = inject(Injector);
  private readonly host = inject<ElementRef<HTMLElement>>(ElementRef);

  protected readonly minPasswordLength = MIN_PASSWORD_LENGTH;

  protected readonly form = inject(NonNullableFormBuilder).group({
    currentPassword: ['', Validators.required],
    newPassword: ['', [Validators.required, Validators.minLength(MIN_PASSWORD_LENGTH)]],
  });

  /** Whether the form was submitted, after which every field shows its errors. */
  private readonly submitted = signal(false);
  /** Why the server didn't accept fields, until they change. */
  private readonly serverErrors = signal<Partial<Record<Field, FieldError>>>({});
  protected readonly busy = signal(false);
  /** Why the last change failed, until the next. */
  protected readonly error = signal<unknown>(null);

  constructor() {
    // A field's answer from the server applies until the field changes.
    for (const field of Object.keys(this.form.controls) as Field[]) {
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
      case 'currentPassword.required':
        return $localize`Enter your current password.`;
      case 'currentPassword.incorrect':
        return $localize`That isn't your current password.`;
      case 'newPassword.required':
        return $localize`Enter a new password.`;
      case 'newPassword.too_short':
        return $localize`Use at least ${error.min}:min: characters.`;
      case 'newPassword.too_long':
        return $localize`Use at most ${error.max}:max: characters.`;
      case 'newPassword.compromised':
        return $localize`This password is on a list of passwords stolen from other sites. Choose another one.`;
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
      this.focusFirstError();
      return;
    }
    this.busy.set(true);
    try {
      const { currentPassword, newPassword } = this.form.getRawValue();
      const answer = await this.reauthentication.run(() =>
        this.http.post('/api/account/password', { currentPassword, newPassword }),
      );
      if (answer !== CANCELLED) {
        this.form.reset();
        this.submitted.set(false);
        this.toasts.show($localize`Password changed. Your other browsers are signed out.`);
      }
    } catch (error) {
      this.failed(error);
    } finally {
      this.busy.set(false);
    }
  }

  private failed(error: unknown): void {
    const response = error instanceof HttpErrorResponse ? error : undefined;
    switch (response?.status) {
      case 422:
        this.serverErrors.set(response.error?.fields ?? {});
        this.focusFirstError();
        return;
      case 429:
        this.error.set(
          tryAgainAfter(
            response,
            $localize`Too many wrong passwords have been entered for your account.`,
            this.locale,
          ),
        );
        return;
      default:
        this.error.set(error);
    }
  }

  private focusFirstError(): void {
    afterNextRender(
      () => this.host.nativeElement.querySelector<HTMLElement>('[aria-invalid="true"]')?.focus(),
      { injector: this.injector },
    );
  }
}
