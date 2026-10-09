# Self-hosted lazy template string, an ordinary variadic struct generator.
# The checker writes a `t"…"` literal as a call of `__make_tstring`, the
# entry point it names by module path, over an interleaved pack of the
# literal segments (as `String`s) and the interpolation snapshots (typed
# values; a place that is not ImplicitlyCopyable arrives pre-formatted as a
# string).  Formatting is deferred: write_to streams the captured elements in
# source order, so print/String() consume a TString through the ordinary
# Writable machinery.
struct TString[*Ts: Movable & Writable](Deinitable, Movable, Writable):
    var storage: Tuple[*Self.Ts]

    def __init__(out self, var *args: *Self.Ts):
        self.storage = Tuple(*args^)

    def write_to(self, mut writer: Some[Writer]):
        comptime for i in range(len(Self.Ts)):
            # The unrolled iterations share one scope, so the ref binding
            # needs a nested block; the ref read keeps non-Copyable
            # captured values legal where a value read would demand a copy.
            if True:
                ref element = self.storage[i]
                writer.write(element)


def __make_tstring[*Ts: Movable & Writable](
    var *args: *Ts, out tstring: TString[*Ts]
):
    """Build the `TString` a `t"…"` literal stands for."""
    tstring = TString(*args^)
