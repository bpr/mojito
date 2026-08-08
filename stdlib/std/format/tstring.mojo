# Self-hosted lazy template string.  A `t"…"` literal desugars during
# whole-program elaboration into a TString construction whose interleaved
# pack captures the literal segments (as compile-time strings) and the
# interpolation snapshots (typed values; non-Copyable places arrive
# pre-formatted as strings).  Formatting is deferred: write_to streams the
# captured elements in source order, so print/String() consume a TString
# through the ordinary Writable machinery.
struct TString[*Ts: Movable & Writable](Movable, Writable):
    var storage: Tuple[*Ts]

    def __init__(out self, var *args: *Ts):
        self.storage = Tuple(*args^)

    def write_to(self, mut writer: Some[Writer]):
        comptime for i in range(len(Ts)):
            # The unrolled iterations share one scope, so the ref binding
            # needs a nested block; the ref read keeps non-Copyable
            # captured values legal where a value read would demand a copy.
            if True:
                ref element = self.storage[i]
                writer.write(element)
