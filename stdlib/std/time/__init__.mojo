"""Time utilities. Mojito ports only the C `timespec` record so far (used by
the filesystem status records); clocks and sleeps are a later slice."""

from .time import _CTimeSpec
