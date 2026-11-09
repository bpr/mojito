# `source_location()` reports its own call's `(`, `call_location()` the call
# site an `@inline(.always)` chain was entered from, `inline_count` frames
# out, and a walk past the outermost frame stays at it, as at the pin. The
# `debug_assert` overloads take upstream's `location:
# Optional[SourceLocation]`.
from std.reflection import SourceLocation, call_location, source_location


@inline(.always)
def caller_site() -> SourceLocation:
    return call_location()


@inline(.always)
def outer_site() -> SourceLocation:
    return call_location[inline_count=2]()


@inline(.always)
def through() -> SourceLocation:
    return outer_site()


@inline(.never)
def saturated() -> SourceLocation:
    return call_location()


def show(loc: SourceLocation):
    print(
        loc.line(),
        loc.column(),
        loc.file_name().endswith("reflection_source_location.mojo"),
    )


def main():
    var here = source_location()
    show(here)
    show(caller_site())
    show(  through())
    show(saturated())
    var none: Optional[SourceLocation] = None
    show(none.or_else(source_location()))
    show(Optional(here).value())
    print(here.prefix("boom").endswith(":38:31: boom"))
    debug_assert(True, location=None)
    debug_assert[assert_mode="safe"](1 == 1, "never", location=here)
