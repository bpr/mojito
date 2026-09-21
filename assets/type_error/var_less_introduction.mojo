# expect: var count
# A var-less introduction (`count = 10` on an undeclared name) is rejected:
# Mojito requires `var` to declare a new variable. The pinned Mojo rejects it
# too since 1.2.0.dev2026092105 ("implicit declaration of 'count' is not
# allowed; add 'var' to declare a new name"); before that it only warned.
def main():
    count = 10
    print(count)
