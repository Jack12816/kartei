// A sample TypeScript module for the extraction tests.

export interface Widget {
  id: number;
  title: string;
}

export type WidgetMap = Record<string, Widget>;

enum Status {
  Draft,
  Published,
}

export namespace Registry {
  export const LIMIT = 100;

  export function register(widget: Widget): void {
    console.log(widget);
  }

  export namespace Cache {
    export function clear(): void {}
  }
}

declare function inspect(widget: Widget): string;

export abstract class Repository {
  abstract find(id: number): Widget;

  count(): number {
    return 0;
  }
}

export class WidgetService {
  private widgets: Widget[] = [];

  add(widget: Widget): void {
    this.widgets.push(widget);
  }

  get size(): number {
    return this.widgets.length;
  }
}

export const MAX_WIDGETS = 25;

export const makeWidget = (title: string): Widget => ({
  id: 1,
  title,
});
