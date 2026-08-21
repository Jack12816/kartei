"""A sample Python module for the extraction tests."""

import functools

VERSION = "1.2.3"
MAX_RETRIES: int = 3
_cache = {}


def plain(value):
    """Return the value untouched."""
    return value


async def fetch(url):
    """Pretend to fetch the URL."""
    return url


@functools.cache
def memoized(value):
    """Return the value, cached."""
    return value


def outer():
    """Hold a nested helper."""

    def inner():
        return 1

    return inner


class Widget:
    """A widget."""

    @property
    def title(self):
        return self._title

    class Meta:
        """Nested widget metadata."""

        def describe(self):
            return "meta"


class Registry(Widget):
    """A registry of widgets."""

    def register(self, widget):
        self.widgets.append(widget)
