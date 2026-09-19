# expect: ambiguous overloaded call
# Two type-pack overloads whose bounds both hold for the arguments: nothing
# ranks one above the other, so the call is ambiguous.


def show[*Ts: Writable](*args: *Ts) -> Int:
    return 1


def show[*Ts: Intable](*args: *Ts) -> Int:
    return 2


def main():
    print(show(1, 2))
