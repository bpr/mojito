# A reference result passed to a `read` parameter binds the parameter to the
# reference the call returned, as at the pin: `show(b.get())` and
# `show(o.value())` copy nothing, so `Loud.__copyinit__` never prints.
struct Loud(Copyable, Movable, Writable):
    var name: String

    def __init__(out self, name: String):
        self.name = name

    def __init__(out self, *, copy: Self):
        self.name = copy.name
        print("copy", self.name)

    def write_to(self, mut writer: Some[Writer]):
        writer.write(self.name)


struct Holder:
    var loud: Loud

    def __init__(out self, var loud: Loud):
        self.loud = loud^

    def get(ref self) -> ref[self.loud] Loud:
        return self.loud


def show(l: Loud):
    print("show", l)


def show_any[T: Writable](value: T):
    print("any", value)


def both(a: Loud, b: Loud):
    print("both", a, b)


def main():
    var h = Holder(Loud("held"))
    show(h.get())
    show_any(h.get())
    both(h.get(), h.get())
    var o = Optional(Loud("opt"))
    show(o.value())
    show_any(o.value())
    print("done")
