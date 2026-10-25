import { Component } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { provideRouter } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { AppLayout } from './app-layout';

@Component({ template: '' })
class Screen {}

describe('AppLayout', () => {
  let harness: RouterTestingHarness;
  let page: HTMLElement;
  let menuButton: HTMLButtonElement;

  beforeEach(async () => {
    TestBed.configureTestingModule({
      providers: [
        provideRouter([
          {
            path: '',
            component: AppLayout,
            children: [
              { path: 'devices', component: Screen },
              { path: 'vpn', component: Screen },
            ],
          },
        ]),
      ],
    });
    harness = await RouterTestingHarness.create('/devices');
    page = harness.fixture.nativeElement;
    menuButton = page.querySelector('.menu-button')!;
  });

  async function toggleMenu(): Promise<void> {
    menuButton.click();
    await harness.fixture.whenStable();
  }

  it('opens and closes the navigation with the menu button', async () => {
    expect(menuButton.getAttribute('aria-expanded')).toBe('false');

    await toggleMenu();
    expect(menuButton.getAttribute('aria-expanded')).toBe('true');
    expect(page.querySelector('#primary-nav')!.classList).toContain('open');

    await toggleMenu();
    expect(menuButton.getAttribute('aria-expanded')).toBe('false');
  });

  it('closes the navigation after navigating', async () => {
    await toggleMenu();

    await harness.navigateByUrl('/vpn');

    expect(menuButton.getAttribute('aria-expanded')).toBe('false');
  });

  it('closes the navigation on Escape and returns focus to the menu button', async () => {
    await toggleMenu();

    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
    await harness.fixture.whenStable();

    expect(menuButton.getAttribute('aria-expanded')).toBe('false');
    expect(document.activeElement).toBe(menuButton);
  });

  it('marks the current screen', () => {
    const current = page.querySelector('[aria-current="page"]');

    expect(current?.textContent?.trim()).toBe('Devices');
  });
});
