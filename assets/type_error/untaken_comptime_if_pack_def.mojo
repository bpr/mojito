# expect: type 'Ts[i]' has no method 'nonexistent'
# A pack element under a `comptime for` index is opaque: only the pack's
# declared bound is known of it, so a member no bound declares is rejected
# inside an untaken `comptime if` arm, from the template. The concrete
# elements of a caller cannot make the body valid.
def show[*Ts: Writable](*args: *Ts):
    comptime for i in range(args.__len__()):
        comptime if i > 100:
            args[i].nonexistent()
        print(args[i])


def main():
    show(1, "two")
