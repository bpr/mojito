# expect: use of uninitialized value 's'
# `print`'s heterogeneous variadic pack reads each argument when it is built,
# so a local moved before the call is used uninitialized.
def main():
    var s = String("hi")
    var t = s^
    print(t)
    print(s)
