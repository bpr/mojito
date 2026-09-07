# `write_repr_to(writer)` on a `Writable`-bounded parameter formats the
# receiver's repr through the writer, as `write_to` formats its str form;
# a slice descriptor writes both forms explicitly.
@fieldwise_init
struct Wrap[T: Writable & Copyable & Deinitable](Writable):
    var value: Self.T
    def write_to(self, mut writer: Some[Writer]):
        writer.write("Wrap(")
        self.value.write_repr_to(writer)
        writer.write(")")

def emit[T: Writable](value: T, mut writer: Some[Writer]):
    value.write_repr_to(writer)

def main():
    print(Wrap(Int(8)))
    print(Wrap(String("hi")))
    print(Wrap(True))
    var slice = Slice(1, 4, None)
    var text = String()
    slice.write_to(text)
    text.write(" | ")
    slice.write_repr_to(text)
    text.write(" | ")
    emit(slice, text)
    print(text)
