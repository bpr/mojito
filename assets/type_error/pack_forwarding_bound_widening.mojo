# expect: cannot unpack a pack of type 'Writable' into a call that expects a pack of type 'Writable & Copyable'
# A forwarded pack carries only its declared bound: the callee's pack may not
# ask for more than the caller's pack guarantees of every element.
def inner[*Ts: Writable & Copyable](*a: *Ts):
    comptime for i in range(a.__len__()):
        print(a[i])


def outer[*Ts: Writable](*a: *Ts):
    inner(*a)


def main():
    outer(1, "two")
