# expect: combining a '**' unpack with other keyword arguments for '**kwargs' is not supported
# A forwarded `**kwargs^` is the callee's keyword collector whole: no keyword
# argument the callee's named parameters leave over may join it.
def inner(var **kwargs: Int) -> Int:
    return len(kwargs)


def relay(var **kwargs: Int) -> Int:
    return inner(c=3, **kwargs^)


def main():
    print(relay(a=1, b=2))
