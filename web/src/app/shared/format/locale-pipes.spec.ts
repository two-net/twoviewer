import { TestBed } from '@angular/core/testing';
import { LocaleDatePipe } from './locale-date-pipe';
import { LocaleNumberPipe } from './locale-number-pipe';
import { USER_LOCALE, UserLocale } from './user-locale';

describe('locale pipes', () => {
  function pipes(locale: UserLocale): { date: LocaleDatePipe; number: LocaleNumberPipe } {
    TestBed.configureTestingModule({ providers: [{ provide: USER_LOCALE, useValue: locale }] });
    return TestBed.runInInjectionContext(() => ({
      date: new LocaleDatePipe(),
      number: new LocaleNumberPipe(),
    }));
  }

  const utc = '2026-10-25T21:30:00Z';

  it('formats dates in the user locale and time zone', () => {
    const { date } = pipes({ locales: ['de-DE'], timeZone: 'Europe/Berlin' });

    expect(date.transform(utc, 'date')).toBe('25.10.2026');
    // 21:30 UTC is 22:30 in Berlin, on the day summer time ends.
    expect(date.transform(utc, 'time')).toBe('22:30');
    expect(date.transform(new Date(utc), 'dateTime')).toBe('25.10.2026, 22:30');
  });

  it('follows the time zone across the date line', () => {
    const { date } = pipes({ locales: ['en-US'], timeZone: 'Asia/Bangkok' });

    expect(date.transform(utc, 'date')).toBe('Oct 26, 2026');
  });

  it('formats numbers in the user locale', () => {
    expect(pipes({ locales: ['de-DE'] }).number.transform(1234.5)).toBe('1.234,5');
  });

  it('uses the first locale the browser knows', () => {
    const { number } = pipes({ locales: ['en-IN', 'en-US'] });

    expect(number.transform(1234567)).toBe('12,34,567');
  });

  it('shows nothing for missing values', () => {
    const { date, number } = pipes({ locales: ['en-US'] });

    expect(date.transform(null)).toBe('');
    expect(date.transform(undefined)).toBe('');
    expect(number.transform(null)).toBe('');
  });
});
