import { Component } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { RouterOutlet, provideRouter } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { AccountMenu } from './account-menu';

@Component({
  imports: [AccountMenu, RouterOutlet],
  template: '<app-account-menu /><router-outlet />',
})
class Shell {}

@Component({ template: '<p class="outside">Elsewhere</p>' })
class Screen {}

describe('AccountMenu', () => {
  let harness: RouterTestingHarness;
  let page: HTMLElement;
  let button: HTMLButtonElement;
  let panel: HTMLElement;

  beforeEach(async () => {
    TestBed.configureTestingModule({
      providers: [
        provideRouter([
          {
            path: '',
            component: Shell,
            children: [
              { path: 'devices', component: Screen },
              { path: 'settings', component: Screen },
            ],
          },
        ]),
      ],
    });
    harness = await RouterTestingHarness.create('/devices');
    page = harness.fixture.nativeElement;
    button = page.querySelector('app-account-menu button')!;
    panel = page.querySelector('#account-menu')!;
  });

  afterEach(() => {
    localStorage.clear();
    document.documentElement.removeAttribute('data-theme');
  });

  async function toggle(): Promise<void> {
    button.click();
    await harness.fixture.whenStable();
  }

  function isOpen(): boolean {
    return button.getAttribute('aria-expanded') === 'true' && panel.classList.contains('open');
  }

  it('opens and closes with the Account button', async () => {
    expect(button.textContent?.trim()).toBe('Account');
    expect(button.getAttribute('aria-controls')).toBe('account-menu');
    expect(isOpen()).toBe(false);

    await toggle();
    expect(isOpen()).toBe(true);

    await toggle();
    expect(isOpen()).toBe(false);
  });

  it('offers the theme and a link to Settings (THM-08)', () => {
    expect(panel.querySelector('app-theme-picker legend')?.textContent?.trim()).toBe('Theme');
    expect(panel.querySelector('a')?.getAttribute('href')).toBe('/settings');
  });

  it('stays open while the theme is chosen', async () => {
    await toggle();

    panel.querySelector<HTMLInputElement>('input[value="dark"]')!.click();
    await harness.fixture.whenStable();

    expect(isOpen()).toBe(true);
    expect(document.documentElement.getAttribute('data-theme')).toBe('dark');
  });

  it('closes on Escape and returns focus to the button', async () => {
    await toggle();

    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
    await harness.fixture.whenStable();

    expect(isOpen()).toBe(false);
    expect(document.activeElement).toBe(button);
  });

  it('closes on a click outside it', async () => {
    await toggle();

    page.querySelector<HTMLElement>('.outside')!.click();
    await harness.fixture.whenStable();

    expect(isOpen()).toBe(false);
  });

  it('closes after navigating', async () => {
    await toggle();

    await harness.navigateByUrl('/settings');

    expect(page.querySelector('app-account-menu button')).toBe(button);
    expect(isOpen()).toBe(false);
  });
});
