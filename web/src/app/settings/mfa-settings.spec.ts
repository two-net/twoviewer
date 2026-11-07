import { provideHttpClient } from '@angular/common/http';
import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { ComponentFixture, TestBed } from '@angular/core/testing';
import { provideRouter } from '@angular/router';
import { Confirmation } from '../shared/confirm/confirmation';
import { ServerConnection } from '../shared/connection/server-connection';
import { FakeSockets, provideFakeSockets } from '../shared/connection/testing';
import { CANCELLED, Reauthentication } from '../shared/reauthentication/reauthentication';
import { Toasts } from '../shared/toast/toasts';
import { MfaSettings, MfaSetup, MfaStatus } from './mfa-settings';

const OFF: MfaStatus = { enabled: false, recoveryCodesLeft: 0 };
const ON: MfaStatus = { enabled: true, enabledAt: '2026-11-08T09:00:00Z', recoveryCodesLeft: 10 };
const SETUP: MfaSetup = {
  secret: 'GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ',
  uri: 'otpauth://totp/TwoViewer:ann%40example.com?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ',
  qrCode: { size: 33, path: 'M4 4h7v1h-7zM4 5h1v1h-1z' },
};
const CODES = Array.from({ length: 10 }, (_, i) => `7k2m-9xqa-4b0d-ztw${i}`);

describe('MfaSettings', () => {
  let fixture: ComponentFixture<MfaSettings>;
  let page: HTMLElement;
  let server: HttpTestingController;
  let sockets: FakeSockets;
  let reauthentication: Reauthentication;

  beforeEach(async () => {
    sockets = new FakeSockets();
    TestBed.configureTestingModule({
      imports: [MfaSettings],
      providers: [
        provideRouter([]),
        provideHttpClient(),
        provideHttpClientTesting(),
        provideFakeSockets(sockets),
      ],
    });
    server = TestBed.inject(HttpTestingController);
    reauthentication = TestBed.inject(Reauthentication);
    fixture = TestBed.createComponent(MfaSettings);
    page = fixture.nativeElement;
    await fixture.whenStable();
  });

  afterEach(() => server.verify());

  /** Lets the component handle a response, which takes a few turns of promises. */
  async function settle(): Promise<void> {
    for (let i = 0; i < 5; i++) {
      await new Promise((resolve) => setTimeout(resolve));
      await fixture.whenStable();
    }
  }

  async function load(status: MfaStatus): Promise<void> {
    server.expectOne({ method: 'GET', url: '/api/mfa' }).flush(status);
    await settle();
  }

  function text(): string {
    return page.textContent!.replace(/\s+/g, ' ').trim();
  }

  function button(label: string): HTMLButtonElement {
    const found = [...page.querySelectorAll('button')].find(
      (button) => button.textContent!.trim() === label,
    );
    if (!found) {
      throw new Error(`no button "${label}"`);
    }
    return found;
  }

  async function click(label: string): Promise<void> {
    button(label).click();
    await settle();
  }

  function codeInput(): HTMLInputElement {
    return page.querySelector<HTMLInputElement>('#mfa-code')!;
  }

  async function typeCode(code: string): Promise<void> {
    codeInput().value = code;
    codeInput().dispatchEvent(new Event('input'));
    await fixture.whenStable();
  }

  /** Turns on the setup, answering it with {@link SETUP}. */
  async function startSetup(): Promise<void> {
    await load(OFF);
    await click('Turn on');
    server.expectOne({ method: 'POST', url: '/api/mfa/setup' }).flush(SETUP);
    await settle();
  }

  function alert(): string {
    return page.querySelector('[role=alert]')?.textContent?.replace(/\s+/g, ' ').trim() ?? '';
  }

  it('says whether MFA is on, and how many recovery codes are left', async () => {
    expect(page.querySelector('app-skeleton')).not.toBeNull();

    await load(ON);

    expect(page.querySelector('app-skeleton')).toBeNull();
    expect(text()).toMatch(/On · Since .+ 10 recovery codes left\./);
    expect(text()).not.toContain('To get new recovery codes');
    expect(button('Turn off')).toBeTruthy();
  });

  it('says how to get new recovery codes when few are left', async () => {
    await load({ ...ON, recoveryCodesLeft: 1 });

    expect(text()).toContain('1 recovery code left.');
    expect(text()).toContain(
      'To get new recovery codes, turn two-factor authentication off and on again.',
    );
  });

  it('shows a QR code and the setup key for the app', async () => {
    await startSetup();

    const qr = page.querySelector('svg.qr')!;
    expect(qr.getAttribute('role')).toBe('img');
    expect(qr.getAttribute('aria-label')).toBe('QR code for your authenticator app');
    expect(qr.getAttribute('viewBox')).toBe('0 0 33 33');
    expect(qr.querySelector('path')!.getAttribute('d')).toBe(SETUP.qrCode.path);
    expect(page.querySelector('.key')!.textContent).toBe('GEZD GNBV GY3T QOJQ GEZD GNBV GY3T QOJQ');
    expect(codeInput().getAttribute('autocomplete')).toBe('one-time-code');
    expect(codeInput().getAttribute('inputmode')).toBe('numeric');
    expect(document.activeElement).toBe(page.querySelector('#mfa-setup-title'));
  });

  it('turns MFA on with a code, then shows the recovery codes once', async () => {
    await startSetup();
    await typeCode(' 123456 ');
    await click('Turn on');

    const request = server.expectOne({ method: 'POST', url: '/api/mfa/confirm' });
    expect(request.request.body).toEqual({ code: '123456' });
    request.flush({ recoveryCodes: CODES });
    await settle();
    await load(ON);

    expect(page.querySelector('svg.qr')).toBeNull();
    const shown = [...page.querySelectorAll('.codes code')].map((code) => code.textContent);
    expect(shown).toEqual(CODES);
    expect(text()).toContain("this is the only time they're shown");
    expect(document.activeElement).toBe(page.querySelector('#mfa-recovery-title'));

    await click("I've saved them");

    expect(page.querySelector('.codes')).toBeNull();
    expect(text()).toContain('10 recovery codes left.');
    expect(
      TestBed.inject(Toasts)
        .toasts()
        .map((toast) => toast.message),
    ).toContain('Two-factor authentication is on.');
  });

  it('copies and downloads the recovery codes', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });
    const blobs: Blob[] = [];
    URL.createObjectURL = vi.fn((blob: Blob) => {
      blobs.push(blob);
      return 'blob:codes';
    });
    URL.revokeObjectURL = vi.fn();
    const clicked: HTMLAnchorElement[] = [];
    vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(function (
      this: HTMLAnchorElement,
    ) {
      clicked.push(this);
    });

    await startSetup();
    await typeCode('123456');
    await click('Turn on');
    server.expectOne('/api/mfa/confirm').flush({ recoveryCodes: CODES });
    await settle();
    await load(ON);

    await click('Copy');
    expect(writeText).toHaveBeenCalledWith(CODES.join('\n'));

    await click('Download');
    expect(clicked.map((link) => link.download)).toEqual(['twoviewer-recovery-codes.txt']);
    const file = await blobs[0].text();
    expect(file).toContain('Recovery codes for TwoViewer:');
    for (const code of CODES) {
      expect(file).toContain(code);
    }
    expect(URL.revokeObjectURL).toHaveBeenCalledWith('blob:codes');
  });

  it('says when the code is wrong', async () => {
    await startSetup();

    await click('Turn on');
    server.expectNone('/api/mfa/confirm');
    expect(page.querySelector('.field-error')!.textContent).toContain(
      'Enter the code that your authenticator app shows.',
    );

    await typeCode('000000');
    await click('Turn on');
    server
      .expectOne('/api/mfa/confirm')
      .flush(
        { error: 'invalid', fields: { code: { code: 'invalid' } } },
        { status: 422, statusText: 'Unprocessable Entity' },
      );
    await settle();

    expect(page.querySelector('.field-error')!.textContent).toContain("That code isn't right.");
    expect(codeInput().getAttribute('aria-invalid')).toBe('true');
    expect(document.activeElement).toBe(codeInput());
    await typeCode('1');
    expect(page.querySelector('.field-error')).toBeNull();
  });

  it('cancels the setup', async () => {
    await startSetup();
    await click('Cancel');

    expect(page.querySelector('svg.qr')).toBeNull();
    expect(button('Turn on')).toBeTruthy();
  });

  it('does nothing when the user cancels re-authenticating', async () => {
    vi.spyOn(reauthentication, 'run').mockResolvedValue(CANCELLED);
    await load(OFF);

    await click('Turn on');

    expect(page.querySelector('svg.qr')).toBeNull();
    expect(alert()).toBe('');
  });

  it('turns MFA off once the user confirms', async () => {
    const confirm = vi.spyOn(TestBed.inject(Confirmation), 'confirm').mockResolvedValue(false);
    await load(ON);

    await click('Turn off');
    expect(confirm).toHaveBeenCalledWith(
      expect.objectContaining({ title: 'Turn off two-factor authentication?' }),
    );
    server.expectNone({ method: 'DELETE', url: '/api/mfa' });

    confirm.mockResolvedValue(true);
    await click('Turn off');
    server
      .expectOne({ method: 'DELETE', url: '/api/mfa' })
      .flush(null, { status: 204, statusText: 'No Content' });
    await settle();
    await load(OFF);

    expect(button('Turn on')).toBeTruthy();
    expect(
      TestBed.inject(Toasts)
        .toasts()
        .map((toast) => toast.message),
    ).toContain('Two-factor authentication is off.');
  });

  it('reloads after a resync and when MFA changes elsewhere', async () => {
    await load(OFF);
    const connection = TestBed.inject(ServerConnection);
    connection.start();

    sockets.last.receive({ type: 'hello', heartbeatTimeoutMs: 8000 });
    await load(OFF);
    sockets.last.receive({ type: 'event', name: 'sign_ins.changed', data: {} });
    server.expectNone('/api/mfa');
    sockets.last.receive({ type: 'event', name: 'mfa.changed', data: {} });
    await load(ON);

    expect(button('Turn off')).toBeTruthy();
    connection.stop();
  });

  it('starts again when MFA was changed in another window during the setup', async () => {
    await startSetup();
    await typeCode('123456');
    await click('Turn on');
    server
      .expectOne('/api/mfa/confirm')
      .flush({ error: 'mfa_enabled' }, { status: 409, statusText: 'Conflict' });
    await settle();
    await load(ON);

    expect(page.querySelector('svg.qr')).toBeNull();
    expect(alert()).toContain('Two-factor authentication was changed in another window.');
    expect(button('Turn off')).toBeTruthy();
  });
});
