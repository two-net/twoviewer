import { Component } from '@angular/core';
import { ComponentFixture, TestBed } from '@angular/core/testing';
import { Theme } from './theme';
import { ThemePicker } from './theme-picker';

@Component({
  imports: [ThemePicker],
  template: '<app-theme-picker /><app-theme-picker />',
})
class TwoPickers {}

describe('ThemePicker', () => {
  afterEach(() => {
    localStorage.clear();
    document.documentElement.removeAttribute('data-theme');
  });

  let fixture: ComponentFixture<TwoPickers>;

  async function render(): Promise<HTMLElement> {
    fixture = TestBed.createComponent(TwoPickers);
    await fixture.whenStable();
    return fixture.nativeElement;
  }

  function radios(picker: Element): HTMLInputElement[] {
    return [...picker.querySelectorAll<HTMLInputElement>('input[type="radio"]')];
  }

  it('offers Light, Dark and System, with System chosen by default (THM-01)', async () => {
    const [picker] = (await render()).querySelectorAll('app-theme-picker');

    expect(picker.querySelector('legend')?.textContent?.trim()).toBe('Theme');
    const labels = [...picker.querySelectorAll('label')].map((label) => label.textContent?.trim());
    expect(labels).toEqual(['Light', 'Dark', 'System']);
    expect(radios(picker).map((radio) => radio.checked)).toEqual([false, false, true]);
  });

  it('applies the chosen option, and every picker shows it', async () => {
    const page = await render();
    const [first, second] = page.querySelectorAll('app-theme-picker');

    radios(first)[1].click();
    await fixture.whenStable();

    expect(TestBed.inject(Theme).option()).toBe('dark');
    expect(document.documentElement.getAttribute('data-theme')).toBe('dark');
    expect(radios(second).map((radio) => radio.checked)).toEqual([false, true, false]);
  });

  it('makes each picker its own radio group', async () => {
    const [first, second] = (await render()).querySelectorAll('app-theme-picker');

    expect(radios(first)[0].name).toBeTruthy();
    expect(radios(first)[0].name).not.toBe(radios(second)[0].name);
  });
});
