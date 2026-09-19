# The same abandonment without any compile-time control flow: the rule is
# about the temporary, not about `comptime if`. Nothing calls `outer`, so its
# body is checked only abstractly.
# expect: '(expression temporary)' abandoned without being explicitly destroyed
def pick[T: ImplicitlyCopyable & Writable](x: T) -> T:
    return x

def outer[T: ImplicitlyCopyable & Writable](x: T):
    print(pick(x))

def main():
    print("unreached by outer")
