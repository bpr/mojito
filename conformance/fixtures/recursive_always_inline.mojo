# Mojito accepts a reached recursive `@always_inline` function, which the
# pin rejects: "function has recursive call to 'always_inline' function".

@always_inline("nodebug")
def spin(x: Int) -> Int:
    if x <= 0:
        return 0
    return spin(x - 1)


def main():
    print(spin(3))
