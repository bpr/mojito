"""Formatted text output (subset of upstream's `format`): the `Writer`
trait. `Writable` is a compiler builtin here."""

from std.string import StringSpan


trait Writer:
    """A destination for formatted text output: `write_string` is the core
    method, and `write` writes each `Writable` argument in turn."""

    def write_string(mut self, string: StringSpan):
        ...

    def write[*Ts: Writable](mut self, *args: *Ts):
        comptime for i in range(args.__len__()):
            args[i].write_to(self)
