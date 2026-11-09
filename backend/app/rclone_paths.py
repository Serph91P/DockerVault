"""Pinned rclone1.72.1 fs/fspath connection-string grammar, without creating a filesystem."""

import re

NAME = re.compile(r"[\w.+@]+(?:[ -]+[\w.+@-]+)*\Z")
PARAM = re.compile(r"[A-Za-z0-9_.]+\Z")


def parse(value):
    """Return (name, overrides, path); name=None denotes an ordinary local path."""
    if type(value) is not str or not value or "\x00" in value:
        raise ValueError("Invalid rclone reference")
    if ":" not in value:
        return None, {}, value
    index = 1 if value.startswith(":") else 0
    while index < len(value) and value[index] not in {":", ","}:
        if value[index] in {"/", "\\"}:
            if value.startswith(":"):
                raise ValueError("Invalid rclone reference")
            return None, {}, value
        index += 1
    name = value[:index]
    if index == len(value) or not NAME.fullmatch(name.lstrip(":")):
        raise ValueError("Invalid rclone reference")
    parameters = {}
    if value[index] == ":":
        return name, parameters, value[index + 1 :]
    index += 1
    while index < len(value):
        start = index
        while index < len(value) and value[index] not in {"=", ":", ","}:
            index += 1
        parameter = value[start:index]
        if index == len(value) or not PARAM.fullmatch(parameter):
            raise ValueError("Invalid rclone reference")
        delimiter = value[index]
        index += 1
        if delimiter == "=":
            if index < len(value) and value[index] in {"'", '"'}:
                quote, characters = value[index], []
                index += 1
                while index < len(value):
                    character = value[index]
                    index += 1
                    if character != quote:
                        characters.append(character)
                    elif index < len(value) and value[index] == quote:
                        characters.append(quote)
                        index += 1
                    else:
                        break
                else:
                    raise ValueError("Invalid rclone reference")
                item = "".join(characters)
                if index == len(value) or value[index] not in {":", ","}:
                    raise ValueError("Invalid rclone reference")
            else:
                start = index
                while index < len(value) and value[index] not in {":", ","}:
                    index += 1
                item = value[start:index]
                if index == len(value):
                    raise ValueError("Invalid rclone reference")
            delimiter = value[index]
            index += 1
        else:
            item = "true"
        parameters[parameter] = item
        if delimiter == ":":
            return name, parameters, value[index:]
    raise ValueError("Invalid rclone reference")


def environment_name(section, field):
    # Go strings.ToUpper uses simple Unicode mappings, not Python's ß -> SS expansion.
    def simple_upper(character):
        upper = character.upper()
        if len(upper) == 1:
            return upper
        title = character.title()
        return title if len(title) == 1 else character

    return "RCLONE_CONFIG_" + "".join(
        simple_upper(character) for character in section + "_" + field.replace("-", "_")
    )
