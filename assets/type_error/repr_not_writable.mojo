# `repr`'s `T: Writable` bound rejects a value that does not conform, as
# overload resolution over the library function does.
# expect: does not conform to trait 'Writable'
struct Q:
    var x: Int

    def __init__(out self):
        self.x = 1


def main():
    print(repr(Q()))
