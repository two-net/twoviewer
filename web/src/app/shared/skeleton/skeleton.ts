import { Component, computed, input } from '@angular/core';

/**
 * Placeholder rows for a list that is loading, such as the devices or the audit log (ANI-08). They
 * shimmer, except with reduced motion. Screen readers hear "Loading…" instead.
 */
@Component({
  selector: 'app-skeleton',
  templateUrl: './skeleton.html',
  styleUrl: './skeleton.scss',
})
export class Skeleton {
  /** How many rows to show: about as many as the list usually has, so that little moves after. */
  readonly rows = input(3);
  protected readonly placeholders = computed(() =>
    Array.from({ length: this.rows() }, (_, i) => i),
  );
}
