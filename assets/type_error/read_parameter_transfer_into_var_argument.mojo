# expect: cannot transfer out of immutable reference
# Handing a read parameter to a `var` parameter with `^` would let the callee
# destroy the caller's value.
def sink(var s: String):
    print(s)


def forward(x: String):
    sink(x^)


def main():
    forward(String("abc"))
