# expect: 'ref' bindings are not supported in 'comptime match'
# A `comptime match` binding is a compile-time value; a `ref` binding is the
# pin's rejection.
def main():
    comptime n = 3
    comptime match n:
        case ref k:
            print(k)
