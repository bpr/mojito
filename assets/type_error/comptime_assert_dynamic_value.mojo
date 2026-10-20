# expect: cannot use a dynamic value in 'comptime assert' expression
# A `comptime assert` condition reads compile-time values alone; a runtime
# local is the pin's rejection.

def main():
    var x = 3
    comptime assert x == 3, "runtime"
    print(x)
