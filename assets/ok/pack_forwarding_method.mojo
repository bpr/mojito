# A pack-keyed method forwards its pack whole into another pack-keyed
# method, alone and after a fixed positional argument.
struct Sink:
    def __init__(out self):
        pass

    def take[*Ts: Writable](self, *a: *Ts):
        comptime for i in range(a.__len__()):
            print(a[i])

    def tagged[*Ts: Writable](self, tag: Int, *a: *Ts):
        print(tag, a.__len__())

    def relay[*Ts: Writable](self, *a: *Ts):
        self.take(*a)
        self.tagged(7, *a)


def main():
    Sink().relay(1, "two")
    Sink().relay(3.5)
