# isspace (non-empty, all Python-space codepoints), is_ascii_digit
# (non-empty, all `0`-`9`), and is_ascii_printable (all `0x20`-`0x7E`; the
# empty string is printable).
def main():
    print(String(" \t\n\r").isspace(), String("").isspace(), String(" a ").isspace(), String("\t").isspace())
    print(String("123").is_ascii_digit(), String("12a").is_ascii_digit(), String("").is_ascii_digit(), String("١").is_ascii_digit())
    print(String("abc ~").is_ascii_printable(), String("a\tb").is_ascii_printable(), String("").is_ascii_printable(), String("é").is_ascii_printable())
    var digits = String("007")
    print(digits.is_ascii_digit(), digits.isspace(), digits.is_ascii_printable(), digits.isupper())
