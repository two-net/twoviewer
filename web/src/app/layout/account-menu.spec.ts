import { provideHttpClient } from '@angular/common/http';
import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { Component } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { Router, RouterOutlet, provideRouter } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { provideFakeSockets } from '../shared/connection/testing';
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
          { path: 'sign-in', component: Screen },
        ]),
        provideHttpClient(),
        provideHttpClientTesting(),
        provideFakeSockets(),
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

  /** Lets the menu handle a response, which takes a few turns of promises. */
  async function settle(): Promise<void> {
    for (let i = 0; i < 5; i++) {
      await new Promise((resolve) => setTimeout(resolve));
      await harness.fixture.whenStable();
    }
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

  it('signs out, then goes to the sign-in page', async () => {
    const server = TestBed.inject(HttpTestingController);
    await toggle();
    const signOut = panel.querySelector<HTMLButtonElement>('button.sign-out')!;
    expect(signOut.textContent?.trim()).toBe('Sign out');

    signOut.click();
    await harness.fixture.whenStable();
    expect(signOut.disabled).toBe(true);
    server.expectOne('/api/sign-out').flush(null, { status: 204, statusText: 'No Content' });
    await settle();

    expect(TestBed.inject(Router).url).toBe('/sign-in');
    server.verify();
  });

  it('says so when signing out fails, and stays signed in', async () => {
    const server = TestBed.inject(HttpTestingController);
    await toggle();

    panel.querySelector<HTMLButtonElement>('button.sign-out')!.click();
    server
      .expectOne('/api/sign-out')
      .error(new ProgressEvent('error'), { status: 0, statusText: 'Unknown Error' });
    await settle();

    expect(TestBed.inject(Router).url).toBe('/devices');
    expect(panel.querySelector('[role=alert]')?.textContent).toContain("can't be reached");
    expect(panel.querySelector<HTMLButtonElement>('button.sign-out')!.disabled).toBe(false);
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
