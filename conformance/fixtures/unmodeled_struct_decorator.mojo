# Mojito parses an unmodeled struct decorator and ignores it; the pin rejects
# a decorator it does not know, and `@value` is one it has removed.
@value
@fieldwise_init
struct Point:
    var x: Int

def main():
    print(Point(3).x)
