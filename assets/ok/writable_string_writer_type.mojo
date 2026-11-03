# The writer `String(x)`, `repr(x)`, and `String.write` hand a struct's
# `write_to` is a `String`, which a body naming its writer's type observes.
from std.reflection.type_info import _unqualified_type_name

@fieldwise_init
struct P(Writable):
    var x: Int

    def write_to(self, mut writer: Some[Writer]):
        writer.write(_unqualified_type_name[type_of(writer)](), ":", self.x)

def main():
    print(String(P(1)))
    print(repr(P(3)))
    var s = String()
    s.write(P(4))
    print(s)
