# expect: type mismatch for argument to 'g'
# A literal converts to an immutable `StaticString`, so it cannot solve an
# origin binder declared `MutOrigin`.
def g[o: MutOrigin](s: StringSpan[o]) -> Int:
    return s.byte_length()


def main():
    print(g("xy"))
