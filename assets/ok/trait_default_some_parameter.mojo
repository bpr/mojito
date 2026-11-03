# A trait default taking `Some[Writer]` is forwarded to by the conformer's
# wrapper, whose own `Some[Writer]` the call deduces from its argument. A
# conformer's method over a binder of its own bounded by the same trait
# (`[W: Writer](mut writer: W)`), or `Some[Writer]` against a default over
# such a binder, is the same signature, so it suppresses the default.


trait Greeter:
    def greet(self, mut writer: Some[Writer]):
        writer.write_string("hi")


trait Named:
    def name[W0: Writer](self, mut writer: W0):
        writer.write_string("default")


@fieldwise_init
struct Plain(Greeter, Named):
    var x: Int


@fieldwise_init
struct Own(Greeter, Named):
    var x: Int

    def greet[W: Writer](self, mut writer: W):
        writer.write_string("own greet")

    def name(self, mut writer: Some[Writer]):
        writer.write_string("own name")


def main():
    var s = String()
    Plain(1).greet(s)
    s.write_string(" ")
    Plain(1).name(s)
    s.write_string(" ")
    Own(2).greet(s)
    s.write_string(" ")
    Own(2).name(s)
    print(s)
