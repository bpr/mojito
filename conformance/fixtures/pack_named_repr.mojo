# Upstream's collection-repr spelling: a `Named` temporary (an origin-bearing
# struct whose origin binder the pack element infers) as an element of
# `FormatStruct.params`.
from std.format._utils import FormatStruct, Named


struct S(Writable):
    var x: Int

    def __init__(out self, x: Int):
        self.x = x

    def write_to(self, mut writer: Some[Writer]):
        var k = self.x + 1
        FormatStruct(writer, "S").params(Named("k", k)).fields(self.x)


def main():
    print(S(3))
