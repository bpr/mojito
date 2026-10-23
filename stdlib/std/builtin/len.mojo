# Upstream's `std/builtin/len.mojo`: the `Sized` and `SizedRaising` traits,
# each requiring `__len__`, and the `len` overload set over them. The prelude
# exports all three, as upstream's does.


trait Sized:
    def __len__(self) -> Int:
        ...


trait SizedRaising:
    def __len__(self) raises -> Int:
        ...


# Upstream spells the parameter `StringSlice`; a free-function overload
# spelled with that alias mangles a key its call does not find (R240).
@doc_hidden
@unavailable(
    "`len(String/StringSlice)` is not supported because Mojo strings are UTF-8"
    " encoded, so a single length is ambiguous: it could mean the number of"
    " UTF-8 bytes, the number of Unicode code points, or the number of"
    " user-visible characters (grapheme clusters). Use `s.byte_length()` or"
    " `len(s.bytes())` for the number of UTF-8 bytes, `len(s.codepoints())` for"
    " Unicode code points, or `len(s.graphemes())` for grapheme clusters."
)
def len(value: StringSpan) -> Int:
    ...


@inline(.always)
def len[T: Sized](value: T) -> Int:
    return value.__len__()


@inline(.always)
def len[T: SizedRaising](value: T) raises -> Int:
    return value.__len__()
