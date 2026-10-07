# A malformed literal `format` template on a path the run never takes: the
# pin parses a literal template at compile time and rejects this program,
# while Mojito parses it at run time, so the program runs.


def show(n: Int):
    if n > 0:
        print("[{:>5}]".format(n))


def main():
    show(0)
    print("ok")
