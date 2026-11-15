# A trait a composition entry lists may not also be opted out with `not`.
# expect: trait 'Base' must not be listed and also opted out with 'not'; remove one of the entries


trait Base:
    pass


trait Other:
    pass


struct Handle(Base & Other, not Base):
    var fd: Int


def main():
    pass
