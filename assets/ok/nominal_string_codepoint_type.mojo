# Codepoint values: conversion to Int, ASCII and UTF-8 width queries,
# equality and ordering, and display. They come from `Codepoint.from_u32`
# here, since a `s[codepoint=i]` index is a Codepoint in Mojito and a
# one-codepoint `StringSpan` upstream (the `string-codepoint-index`
# conformance case).
def main():
    try:
        var g: Codepoint = Codepoint.from_u32(0x67).value()
        var e = Codepoint.from_u32(0xE9).value()
        var face = Codepoint.from_u32(0x1F642).value()
        print(Int(g), Int(e), Int(face))
        print(g.is_ascii(), e.is_ascii())
        print(g.utf8_byte_length(), e.utf8_byte_length(), face.utf8_byte_length())
        print(g == g, g == e, g != e)
        print(g < e, face > e, g <= g, e >= face)
        print(g, e, face)
    except:
        print("unexpected")
    print("done")
