# expect: type mismatch for default value of 's'
# A literal converts to an immutable `StaticString`, so its default cannot
# solve an origin binder declared `MutOrigin`.
def g[o: MutOrigin](s: StringSpan[o] = "xy") -> Int:
    return s.byte_length()


def main():
    print(g())
