import { Component, signal } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { Skeleton } from './skeleton';

@Component({
  imports: [Skeleton],
  template: '<app-skeleton [rows]="rows()" />',
})
class List {
  readonly rows = signal(5);
}

describe('Skeleton', () => {
  it('shows three rows by default', async () => {
    const fixture = TestBed.createComponent(Skeleton);
    await fixture.whenStable();

    expect(fixture.nativeElement.querySelectorAll('.row').length).toBe(3);
  });

  it('shows as many rows as asked for', async () => {
    const fixture = TestBed.createComponent(List);
    await fixture.whenStable();
    const page: HTMLElement = fixture.nativeElement;
    expect(page.querySelectorAll('.row').length).toBe(5);

    fixture.componentInstance.rows.set(2);
    await fixture.whenStable();

    expect(page.querySelectorAll('.row').length).toBe(2);
  });

  it('says that the list is loading, and hides the rows from assistive technologies', async () => {
    const fixture = TestBed.createComponent(Skeleton);
    await fixture.whenStable();
    const status: HTMLElement = fixture.nativeElement.querySelector('[role="status"]');

    expect(status.textContent?.trim()).toBe('Loading…');
    const rows = [...status.querySelectorAll('.row')];
    expect(rows.every((row) => row.getAttribute('aria-hidden') === 'true')).toBe(true);
  });
});
