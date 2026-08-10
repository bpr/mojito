def show(prefix: Int, var **options: Int):
    print(prefix, len(options))


def relay(var **options: Int):
    show(prefix=7, **options^)


def main():
    relay(left=20, right=22)
