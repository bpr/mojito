"""Time utilities (subset): the C `timespec` record."""

comptime _NSEC_PER_SEC = 1_000_000_000


# C `struct timespec` as glibc lays it out (two 64-bit words). Upstream's
# field name for the subsecond word is kept (nanoseconds on Linux).
struct _CTimeSpec(Copyable, Defaultable, Movable, Writable):
    var tv_sec: Int
    var tv_subsec: Int

    def __init__(out self):
        self.tv_sec = 0
        self.tv_subsec = 0

    def as_nanoseconds(self) -> Int:
        return self.tv_sec * _NSEC_PER_SEC + self.tv_subsec

    def write_to(self, mut writer: Some[Writer]):
        writer.write(self.as_nanoseconds(), "ns")
