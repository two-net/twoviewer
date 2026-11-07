import { Component } from '@angular/core';
import { ComponentFixture, TestBed } from '@angular/core/testing';
import { Motion } from './motion';
import { MotionPicker } from './motion-picker';

@Component({
  imports: [MotionPicker],
  template: '<app-motion-picker /><app-motion-picker />',
})
class TwoPickers {}

describe('MotionPicker', () => {
  afterEach(() => {
    localStorage.clear();
    document.documentElement.removeAttribute('data-motion');
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

  it('offers System, Full and Reduced, with System chosen by default (ANI-05)', async () => {
    const [picker] = (await render()).querySelectorAll('app-motion-picker');

    expect(picker.querySelector('legend')?.textContent?.trim()).toBe('Motion');
    const labels = [...picker.querySelectorAll('label')].map((label) => label.textContent?.trim());
    expect(labels).toEqual(['System', 'Full', 'Reduced']);
    expect(radios(picker).map((radio) => radio.checked)).toEqual([true, false, false]);
  });

  it('applies the chosen option, and every picker shows it', async () => {
    const page = await render();
    const [first, second] = page.querySelectorAll('app-motion-picker');

    radios(first)[2].click();
    await fixture.whenStable();

    expect(TestBed.inject(Motion).option()).toBe('reduced');
    expect(document.documentElement.getAttribute('data-motion')).toBe('reduced');
    expect(radios(second).map((radio) => radio.checked)).toEqual([false, false, true]);
  });

  it('makes each picker its own radio group', async () => {
    const [first, second] = (await render()).querySelectorAll('app-motion-picker');

    expect(radios(first)[0].name).toBeTruthy();
    expect(radios(first)[0].name).not.toBe(radios(second)[0].name);
  });
});
