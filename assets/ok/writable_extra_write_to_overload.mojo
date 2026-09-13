# A `write_to` whose parameter is not a `Writer` is an ordinary overload: the
# struct displays through the reflective `Name(field=...)` default.
@fieldwise_init
struct BrokenWritable(Writable):
    var value: Int

    def write_to(self, mut writer: String):
        writer = String(self.value)

def main():
    print(String(BrokenWritable(1)))
    var text = String()
    BrokenWritable(2).write_to(text)
    print(text)
