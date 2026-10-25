import { Pipe, PipeTransform, inject } from '@angular/core';
import { USER_LOCALE } from './user-locale';

/**
 * Formats a number in the user's locale (A11Y-04), as in `{{ count | localeNumber }}`. Use it
 * instead of Angular's `number`, `percent` and `currency` pipes, which format for the interface
 * language.
 */
@Pipe({ name: 'localeNumber' })
export class LocaleNumberPipe implements PipeTransform {
  private readonly locale = inject(USER_LOCALE);

  transform(value: number | null | undefined, options?: Intl.NumberFormatOptions): string {
    if (value === null || value === undefined) {
      return '';
    }
    return new Intl.NumberFormat(this.locale.locales, options).format(value);
  }
}
