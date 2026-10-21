# expect: 'var' bindings are not supported in 'comptime match'
# A `comptime match` binding is a compile-time value; a `var` binding is the
# pin's rejection.
def main():
    comptime n = 3
    comptime match n:
        case var k:
            print(k)
