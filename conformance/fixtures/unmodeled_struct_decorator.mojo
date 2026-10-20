# `@value` is a decorator the pin has removed, so both compilers reject it as
# any decorator unknown at its site: "use of unknown declaration 'value'".
@value
@fieldwise_init
struct Point:
    var x: Int

def main():
    print(Point(3).x)
