import { Pipe, PipeTransform, inject } from '@angular/core';
import { USER_LOCALE } from './user-locale';

export type DateFormat = 'date' | 'time' | 'dateTime';

const OPTIONS: Record<DateFormat, Intl.DateTimeFormatOptions> = {
  date: { dateStyle: 'medium' },
  time: { timeStyle: 'short' },
  dateTime: { dateStyle: 'medium', timeStyle: 'short' },
};

/**
 * Formats a point in time in the user's locale and time zone (A11Y-04), as in
 * `{{ device.lastSeenAt | localeDate }}`. Use it instead of Angular's `date` pipe, which formats
 * for the interface language. The server sends times in UTC (DB-14).
 */
@Pipe({ name: 'localeDate' })
export class LocaleDatePipe implements PipeTransform {
  private readonly locale = inject(USER_LOCALE);
  private readonly formatters = new Map<DateFormat, Intl.DateTimeFormat>();

  transform(
    value: Date | string | number | null | undefined,
    format: DateFormat = 'dateTime',
  ): string {
    if (value === null || value === undefined || value === '') {
      return '';
    }
    let formatter = this.formatters.get(format);
    if (!formatter) {
      formatter = new Intl.DateTimeFormat(this.locale.locales, {
        ...OPTIONS[format],
        timeZone: this.locale.timeZone,
      });
      this.formatters.set(format, formatter);
    }
    return formatter.format(new Date(value));
  }
}
