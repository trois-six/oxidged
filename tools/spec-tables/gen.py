#!/usr/bin/env python3
"""Generate the GEDCOM 5.5.1, 7.0 and 7.1 specification tables of the crate
(src/spec/tables.rs), which the validator, the conformance repair and the
test suite's independent checker read, and the ANSEL table of the suite
(tests/conformance/support/ansel_table.rs).

Standard library only. The output is facts only (tags, structure types,
cardinalities, payload types, enumeration values, calendars and months) and is
deterministic, so a regeneration diff shows exactly what changed upstream.
The layout is compact: structure types are numbered, tags are indices into
one shared tag list, and each version's structure names share one string.

Inputs (pinned in tests/fixtures/corpora.lock.tsv, fetched by
tools/fetch-corpora.sh into target/corpora/spec/):

* GEDCOM 7.0: FamilySearch/GEDCOM `extracted-files/` of the latest release,
  7.0.18 (tag v7.0.18, 2f0b863, Apache-2.0): substructures.tsv,
  cardinalities.tsv, payloads.tsv, enumerations.tsv, enumerationsets.tsv and
  tags/cal-*.
* GEDCOM 7.1, a draft: the same files on the `v7.1` branch at c451779, its
  head (Apache-2.0).
* GEDCOM 5.5.1: cacack/gedcom-go `testdata/spec/gedcom-5.5.1/*.tsv` at 3c6d42d
  (MIT), a transcription of the 5.5.1 PDF, completed and corrected by
  `551-errata.tsv` (this directory), which cites the PDF page of each row.

Usage:
    tools/spec-tables/gen.py [--inputs DIR] [--out PATH] [--check]
    tools/spec-tables/gen.py [--inputs DIR] --crosscheck-551 REGISTRIES

DIR defaults to $CORPORA_DIR/spec, or target/corpora/spec, as tools/fetch-corpora.sh
lays it out.

`--check` regenerates in memory and exits 1 when the committed file differs
(used by the opt-in `spec_tables_are_current` test).
"""

import argparse
import csv
import hashlib
import os
import re
import sys

V7 = 'https://gedcom.io/terms/v7/'
V71 = 'https://gedcom.io/terms/v7.1/'
ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
DEFAULT_OUT = os.path.join(ROOT, 'src', 'spec', 'tables.rs')
ANSEL_OUT = os.path.join(ROOT, 'tests', 'conformance', 'support', 'ansel_table.rs')
ERRATA = os.path.join(os.path.dirname(os.path.abspath(__file__)), '551-errata.tsv')

SOURCES = {
    'fs70': ('FamilySearch/GEDCOM v7.0.18', '2f0b863b5a42275bd2b3ca315900cf779e42f28b', 'Apache-2.0'),
    'fs71': ('FamilySearch/GEDCOM (v7.1 branch)', 'c4517798b163500be5b994f601c51940f3c21d1c', 'Apache-2.0'),
    'gedcom-go': ('cacack/gedcom-go', '3c6d42de6e1c4fc487a8438cc8aaa892578ac97e', 'MIT'),
}
CALENDARS = ['GREGORIAN', 'JULIAN', 'FRENCH_R', 'HEBREW']

# Payload kinds, as Rust `Pay` variants.
PAY_7 = {
    '': 'None',
    'Y|<NULL>': 'Y',
    'http://www.w3.org/2001/XMLSchema#string': 'Text',
    'http://www.w3.org/2001/XMLSchema#nonNegativeInteger': 'Int',
    'http://www.w3.org/2001/XMLSchema#Language': 'Lang',
    'http://www.w3.org/2001/XMLSchema#anyURI': 'Uri',
    'http://www.w3.org/ns/dcat#mediaType': 'MediaType',
    V7 + 'type-Age': 'Age',
    V7 + 'type-Date': 'Date',
    V7 + 'type-Date#exact': 'DateExact',
    V7 + 'type-Date#period': 'DatePeriod',
    V7 + 'type-FilePath': 'FilePath',
    V7 + 'type-Latitude': 'Lat',
    V7 + 'type-Longitude': 'Long',
    V7 + 'type-List#Text': 'ListText',
    V7 + 'type-Name': 'Name',
    V7 + 'type-TagDef': 'TagDef',
    V7 + 'type-Time': 'Time',
}

# 5.5.1 primitives with a dedicated grammar.
PAY_551 = {
    'DATE_VALUE': 'Date',
    'DATE_LDS_ORD': 'Date',
    'DATE_PERIOD': 'DatePeriod',
    'DATE_EXACT': 'DateExact',
    'CHANGE_DATE': 'DateExact',
    'TRANSMISSION_DATE': 'DateExact',
    'PUBLICATION_DATE': 'DateExact',
    'AGE_AT_EVENT': 'Age',
    'TIME_VALUE': 'Time',
    'COUNT_OF_CHILDREN': 'Int',
    'COUNT_OF_MARRIAGES': 'Int',
    'PLACE_LATITUDE': 'Lat',
    'PLACE_LONGITUDE': 'Long',
    'NAME_PERSONAL': 'Name',
}


def sha256(path):
    with open(path, 'rb') as f:
        return hashlib.sha256(f.read()).hexdigest()


# The first cell of the header row of each input TSV file. FamilySearch's
# files have one only after 7.0.18 (its release files start with data).
TSV_HEADERS = {'superstructure', 'structure', 'set', 'primitive', 'block', 'kind'}


def read_tsv(path):
    """The data rows of a TSV file: a header row, when there is one, is left
    out."""
    with open(path, newline='', encoding='utf-8') as f:
        rows = list(csv.reader(f, delimiter='\t', quoting=csv.QUOTE_NONE))
    if rows and rows[0] and rows[0][0] in TSV_HEADERS:
        return rows[1:]
    return rows


def short7(uri):
    for p in (V71, V7):
        if uri.startswith(p):
            return uri[len(p):]
    return uri


def enum_tag(uri):
    """The payload spelling of an enumeration value URI."""
    name = short7(uri)
    if name.startswith('enum-'):
        name = name[len('enum-'):]
    elif name.startswith('enumset-'):  # 7.1 draft spells a few values this way
        name = name[len('enumset-'):]
    return name.rsplit('-', 1)[-1]


def card(text):
    m = re.fullmatch(r'\{?(\d+):(\d+|M)\}?', text.strip())
    if not m:
        raise ValueError(f'bad cardinality {text!r}')
    return int(m[1]), (0 if m[2] == 'M' else int(m[2]))


def yaml_list(path, key):
    """Items of a top-level `key:` list in one of the extracted tags/*.yaml files."""
    out, inside = [], False
    with open(path, encoding='utf-8') as f:
        for line in f:
            line = line.rstrip()
            if re.match(rf'^{key}:\s*\[\]\s*$', line):
                return []
            if re.match(rf'^{key}:\s*$', line):
                inside = True
                continue
            if inside:
                m = re.match(r'^\s+-\s+"?([^"]+?)"?$', line)
                if m:
                    out.append(m[1])
                elif line.strip():
                    break
    return out


def gen_v7(base, label):
    d = os.path.join(base, 'extracted-files')
    files = {n: os.path.join(d, n + '.tsv') for n in
             ('substructures', 'cardinalities', 'payloads', 'enumerations', 'enumerationsets')}
    cards = {(short7(a), short7(b)): card(c) for a, b, c in read_tsv(files['cardinalities'])}
    subs = []
    for sup, tag, typ in read_tsv(files['substructures']):
        sup, typ = short7(sup), short7(typ)
        if tag == 'CONT':
            continue
        if sup == '':
            mn, mx = (1, 1) if tag in ('HEAD', 'TRLR') else (0, 0)
        else:
            mn, mx = cards[(sup, typ)]
        subs.append((sup, tag, typ, mn, mx))
    enum_of = {short7(s): short7(e) for s, e in read_tsv(files['enumerations'])}
    sets = {}
    for s, v in read_tsv(files['enumerationsets']):
        sets.setdefault(short7(s), []).append(enum_tag(v))
    pays = []
    for typ, p in read_tsv(files['payloads']):
        typ = short7(typ)
        if p.startswith('@<') and p.endswith('>@'):
            kind = f'Ptr("{short7(p[2:-2])}")'
        elif p in (V7 + 'type-Enum',):
            kind = f'Enum("{enum_of[typ]}")'
        elif p == V7 + 'type-List#Enum':
            kind = f'ListEnum("{enum_of[typ]}")'
        elif p in PAY_7:
            kind = PAY_7[p]
        else:
            raise ValueError(f'{label}: unknown payload type {p!r} for {typ}')
        pays.append((typ, kind))
    cals = []
    for c in CALENDARS:
        path = os.path.join(d, 'tags', 'cal-' + c)
        months = [short7(m).split('month-', 1)[-1] for m in yaml_list(path, 'months')]
        epochs = yaml_list(path, 'epochs')
        cals.append((c, months, epochs))
    inputs = [files[n] for n in sorted(files)] + [os.path.join(d, 'tags', 'cal-' + c) for c in CALENDARS]
    return subs, pays, sorted((k, sorted(set(v)), False) for k, v in sets.items()), cals, inputs


def gen_v551(base):
    files = {n: os.path.join(base, n + '.tsv') for n in
             ('substructures', 'cardinalities', 'payloads', 'primitives')}
    errata = [r for r in read_tsv(ERRATA) if r and not r[0].startswith('#')]
    alias = {r[1]: r[2] for r in errata if r[0] == 'primitive-alias'}
    enums = {}
    for prim, _size, values in read_tsv(files['primitives']):
        if values:
            enums[prim] = (values.split('|'), False)
    for r in errata:
        if r[0] in ('enum', 'enum-open'):
            enums[r[1]] = (r[2].split('|'), r[0] == 'enum-open')
    cards = {(a, b): card(c) for a, b, c in read_tsv(files['cardinalities'])}
    subs = []
    for sup, tag, typ in read_tsv(files['substructures']):
        if tag in ('CONT', 'CONC'):
            continue
        mn, mx = cards[(sup, typ)]
        subs.append((sup, tag, typ, mn, mx))
    for r in errata:
        if r[0] == 'substructure':
            sup, tag, typ, c = r[1], r[2].split('|')[0], r[2].split('|')[1], r[2].split('|')[2]
            subs.append((sup, tag, typ, *card(c)))
    payload_errata = {r[1]: r[2] for r in errata if r[0] == 'payload'}
    pays = []
    for typ, p in read_tsv(files['payloads']):
        p = payload_errata.get(typ, p).strip()
        if p in ('', '<NULL>'):
            kind = 'None'
        elif p == '[Y|<NULL>]':
            kind = 'Y'
        else:
            m = re.search(r'@<XREF:([A-Z]+)>@', p)
            if m:
                # `[ @<XREF:REPO>@ | <NULL>]` (p.27) admits no pointer.
                kind = f'{"NullPtr" if "<NULL>" in p else "Ptr"}("{m[1]}")'
            else:
                m = re.search(r'<([A-Z_]+)>?', p)
                if not m:
                    raise ValueError(f'5.5.1: bad payload {p!r} for {typ}')
                prim = alias.get(m[1], m[1])
                if prim in PAY_551:
                    kind = PAY_551[prim]
                elif prim in enums:
                    kind = f'Enum("{prim}")'
                else:
                    kind = 'Text'
        pays.append((typ, kind))
    cals = []
    for r in errata:
        if r[0] == 'calendar':
            months = r[2].split('|') if r[2] else []
            epochs = r[3].split('|') if r[3] else []
            cals.append((r[1], months, epochs))
    enum_rows = sorted((k, v[0], v[1]) for k, v in enums.items())
    inputs = [files[n] for n in sorted(files)] + [ERRATA]
    return subs, pays, enum_rows, cals, inputs


# ANSEL (ANSI/NISO Z39.47) to Unicode. The GEDCOM 5.5.1 subset is Appendix C
# of the specification (pp. 97-100); the rest of the repertoire, and the
# Unicode code points, follow the Library of Congress MARC-8 mapping of the
# same standard. GEDCOM spells eszett CF (p. 100); MARC-8 uses C7.
ANSEL_SPACING = [
    (0xA1, 0x0141, 'slash L uppercase'), (0xA2, 0x00D8, 'slash O uppercase'),
    (0xA3, 0x0110, 'slash D uppercase'), (0xA4, 0x00DE, 'thorn uppercase'),
    (0xA5, 0x00C6, 'ligature AE uppercase'), (0xA6, 0x0152, 'ligature OE uppercase'),
    (0xA7, 0x02B9, 'soft sign'), (0xA8, 0x00B7, 'middle dot'), (0xA9, 0x266D, 'musical flat'),
    (0xAA, 0x00AE, 'registered trademark'), (0xAB, 0x00B1, 'plus or minus'),
    (0xAC, 0x01A0, 'O hook uppercase'), (0xAD, 0x01AF, 'U hook uppercase'), (0xAE, 0x02BC, 'alif'),
    (0xB0, 0x02BB, 'ayn'), (0xB1, 0x0142, 'slash l lowercase'), (0xB2, 0x00F8, 'slash o lowercase'),
    (0xB3, 0x0111, 'slash d lowercase'), (0xB4, 0x00FE, 'thorn lowercase'),
    (0xB5, 0x00E6, 'ligature ae lowercase'), (0xB6, 0x0153, 'ligature oe lowercase'),
    (0xB7, 0x02BA, 'hard sign'), (0xB8, 0x0131, 'dotless i lowercase'), (0xB9, 0x00A3, 'British pound'),
    (0xBA, 0x00F0, 'eth'), (0xBC, 0x01A1, 'o hook lowercase'), (0xBD, 0x01B0, 'u hook lowercase'),
    (0xC0, 0x00B0, 'degree sign'), (0xC1, 0x2113, 'script small l'), (0xC2, 0x2117, 'sound recording copyright'),
    (0xC3, 0x00A9, 'copyright mark'), (0xC4, 0x266F, 'musical sharp'), (0xC5, 0x00BF, 'inverted question mark'),
    (0xC6, 0x00A1, 'inverted exclamation mark'), (0xC7, 0x00DF, 'eszett (MARC-8)'), (0xC8, 0x20AC, 'euro sign'),
    (0xCF, 0x00DF, 'eszett (GEDCOM, p. 100)'),
]
ANSEL_MARKS = [
    (0xE0, 0x0309, 'hook above'), (0xE1, 0x0300, 'grave accent'), (0xE2, 0x0301, 'acute accent'),
    (0xE3, 0x0302, 'circumflex accent'), (0xE4, 0x0303, 'tilde'), (0xE5, 0x0304, 'macron'),
    (0xE6, 0x0306, 'breve'), (0xE7, 0x0307, 'dot above'), (0xE8, 0x0308, 'umlaut (diaeresis)'),
    (0xE9, 0x030C, 'hacek'), (0xEA, 0x030A, 'circle above'), (0xEB, 0xFE20, 'ligature, first half'),
    (0xEC, 0xFE21, 'ligature, second half'), (0xED, 0x0315, 'high comma, off center'),
    (0xEE, 0x030B, 'double acute accent'), (0xEF, 0x0310, 'candrabindu'), (0xF0, 0x0327, 'cedilla'),
    (0xF1, 0x0328, 'right hook, ogonek'), (0xF2, 0x0323, 'dot below'), (0xF3, 0x0324, 'double dot below'),
    (0xF4, 0x0325, 'circle below'), (0xF5, 0x0333, 'double underscore'), (0xF6, 0x0332, 'underscore'),
    (0xF7, 0x0326, 'left hook (comma below)'), (0xF8, 0x031C, 'right cedilla'), (0xF9, 0x032E, 'upadhmaniya'),
    (0xFA, 0xFE22, 'double tilde, first half'), (0xFB, 0xFE23, 'double tilde, second half'),
    (0xFE, 0x0313, 'high comma, centered'),
]
# Marks stacked on one base (ANSEL writes every mark before the base).
ANSEL_STACKED = [
    ([0xE3, 0xE1], 'a'), ([0xE3, 0xE2], 'e'), ([0xF2, 0xE3], 'a'), ([0xE6, 0xE2], 'a'),
    ([0xE8, 0xE5], 'u'), ([0xF2, 0xE5], 'l'), ([0xE4, 0xE2], 'o'), ([0xE0, 0xE3], 'o'),
]


def rs_char(c):
    return "'\\u{%x}'" % ord(c)


def rs_ustr(s):
    return '"' + ''.join('\\u{%x}' % ord(c) if ord(c) > 0x7E else c for c in s) + '"'


def generate_ansel():
    import unicodedata
    letters = [chr(c) for c in range(ord('A'), ord('Z') + 1)] + [chr(c) for c in range(ord('a'), ord('z') + 1)]
    out = ['//! GENERATED by `tools/spec-tables/gen.py`; do not edit by hand.',
           '//!',
           '//! ANSEL (Z39.47) to Unicode: the GEDCOM 5.5.1 subset of Appendix C (pp. 97-100) and the rest',
           '//! of the repertoire after the Library of Congress MARC-8 mapping. Compositions are Unicode NFC,',
           '//! which the Unicode normalization stability policy keeps the same in every later version.',
           '',
           '/// Spacing characters: (byte, character, name).',
           'pub static SPACING: &[(u8, char, &str)] = &[']
    for b, cp, name in ANSEL_SPACING:
        out.append(f'    (0x{b:02X}, {rs_char(chr(cp))}, "{name}"),')
    out += ['];', '', '/// Non-spacing marks, written before their base: (byte, combining character, name).',
            'pub static MARKS: &[(u8, char, &str)] = &[']
    for b, cp, name in ANSEL_MARKS:
        out.append(f'    (0x{b:02X}, {rs_char(chr(cp))}, "{name}"),')
    out += ['];', '', '/// Every mark on every ASCII letter: (mark byte, base, NFC text).',
            'pub static COMPOSED: &[(u8, char, &str)] = &[']
    for b, cp, _ in ANSEL_MARKS:
        for l in letters:
            out.append(f"    (0x{b:02X}, '{l}', {rs_ustr(unicodedata.normalize('NFC', l + chr(cp)))}),")
    out += ['];', '', '/// Several marks on one base: (mark bytes in ANSEL order, base, NFC text).',
            'pub static STACKED: &[(&[u8], char, &str)] = &[']
    marks = dict((b, cp) for b, cp, _ in ANSEL_MARKS)
    for bs, l in ANSEL_STACKED:
        text = unicodedata.normalize('NFC', l + ''.join(chr(marks[b]) for b in bs))
        out.append(f"    (&[{', '.join('0x%02X' % b for b in bs)}], '{l}', {rs_ustr(text)}),")
    out += ['];', '']
    return '\n'.join(out)


def rs_str(s):
    return '"' + s.replace('\\', '\\\\').replace('"', '\\"') + '"'


# Payload kinds, as `schema::Kind` variants, from the kind strings above.
KINDS = {
    'None': 'None', 'Y': 'Y', 'Text': 'Text', 'ListText': 'ListText', 'Int': 'Int',
    'Name': 'Name', 'Date': 'Date', 'DateExact': 'DateExact', 'DatePeriod': 'DatePeriod',
    'Age': 'Age', 'Time': 'Time', 'Lang': 'Lang', 'MediaType': 'MediaType',
    'FilePath': 'FilePath', 'Uri': 'Uri', 'Lat': 'Lat', 'Long': 'Long', 'TagDef': 'TagDef',
    'Ptr': 'Pointer', 'NullPtr': 'NullablePointer', 'Enum': 'Enum', 'ListEnum': 'ListEnum',
}


def parse_kind(kind):
    m = re.fullmatch(r'(\w+)(?:\("([^"]*)"\))?', kind)
    return KINDS[m[1]], m[2]


class Schema:
    """One version's tables in the crate's compact layout."""

    def __init__(self, label, version, subs, pays, enums, cals, v71_types=()):
        self.label, self.version, self.cals = label, version, cals
        self.enums = sorted(enums)
        enum_ids = {name: i for i, (name, _, _) in enumerate(self.enums)}
        tag_of = {}
        for sup, tag, typ, _, _ in subs:
            if tag_of.setdefault(typ, tag) != tag:
                raise ValueError(f'{label}: {typ} has two tags')
        # Id 0 is the dataset: its substructures are the records and the
        # header and trailer pseudo-records.
        self.names = [''] + sorted(tag_of)
        self.ids = {n: i for i, n in enumerate(self.names)}
        self.tag_of = dict(tag_of, **{'': ''})
        self.v71 = set(v71_types)
        pay = dict(pays)
        record_by_tag = {tag: typ for sup, tag, typ, _, _ in subs if sup == ''}
        self.kind = {}
        for typ in self.names:
            kind, arg = parse_kind(pay.get(typ, 'None')) if typ else ('None', None)
            if kind in ('Pointer', 'NullablePointer'):
                # 7.x names the record type, 5.5.1 the record tag.
                target = arg if arg in self.ids else record_by_tag[arg]
                arg = self.ids[target]
            elif kind in ('Enum', 'ListEnum'):
                arg = enum_ids[arg]
            else:
                arg = 0
            self.kind[typ] = (kind, arg)
        self.subs = {n: [] for n in self.names}
        for sup, tag, typ, mn, mx in subs:
            if mn > 255 or mx > 255:
                raise ValueError(f'{label}: cardinality out of range')
            self.subs[sup].append((tag, typ, mn, mx))
        for v in self.subs.values():
            v.sort()
            if len(v) > 255:
                raise ValueError(f'{label}: more than 255 substructures')

    def tags(self):
        return {t for t in self.tag_of.values() if t}

    def emit(self, name, tag_ids):
        out = [f'/// GEDCOM {self.version}.', f'pub(crate) static {name}: Schema = Schema {{',
               f'    version: {rs_str(self.version)},']
        offsets, text = [], ''
        for n in self.names:
            offsets.append(len(text))
            text += n
        if len(text) > 0xFFFF:
            raise ValueError('names too long for 16-bit offsets')
        out.append('    names: concat!(')
        line = ''
        for n in self.names:
            if len(line) + len(n) > 88:
                out.append(f'        {rs_str(line)},')
                line = ''
            line += n
        out.append(f'        {rs_str(line)},')
        out.append('    ),')
        out.append('    structs: &[')
        start = 0
        for i, n in enumerate(self.names):
            kind, arg = self.kind[n]
            tag = tag_ids[self.tag_of[n]] if n else 0
            flags = 'URI71' if n in self.v71 else '0'
            out.append(f'        d({offsets[i]}, {len(n)}, {tag}, K::{kind}, {arg}, {start}, '
                       f'{len(self.subs[n])}, {flags}), // {i} {n or "(dataset)"}')
            start += len(self.subs[n])
        out.append('    ],')
        out.append('    subs: &[')
        for n in self.names:
            for tag, typ, mn, mx in self.subs[n]:
                out.append(f'        u({self.ids[typ]}, {mn}, {mx}), // {n or "(dataset)"} {tag}')
        out.append('    ],')
        out.append('    enums: &[')
        for k, vals, open_ in self.enums:
            out.append(f'        e({rs_str(k)}, &[{", ".join(rs_str(v) for v in vals)}], {str(open_).lower()}),')
        out.append('    ],')
        out.append('    #[cfg(test)]')
        out.append('    calendars: &[')
        for c, months, epochs in self.cals:
            out.append(f'        c({rs_str(c)}, &[{", ".join(rs_str(m) for m in months)}], '
                       f'&[{", ".join(rs_str(e) for e in epochs)}]),')
        out.append('    ],')
        out.append('};')
        return '\n'.join(out)


HEADER = """//! GENERATED by `tools/spec-tables/gen.py`; do not edit by hand.
//!
//! Facts only: structure types, tags, cardinalities, payload types,
//! enumeration values, calendars and months of GEDCOM 5.5.1, 7.0 and 7.1.
//! The types and the layout are described in `schema.rs`.
//!
//! Sources (see NOTICE):
{sources}
//!
//! Input SHA-256:
{hashes}

#[cfg(test)]
use super::schema::c;
use super::schema::{{d, e, u, Kind as K, Schema, URI71}};

/// Every tag of the tables, sorted; structure definitions refer to a tag by
/// its index here.
pub(crate) static TAGS: [&str; {ntags}] = [
{tags}
];
"""


def v71_types(base):
    """The structure types the 7.1 tables name with a v7.1 URI."""
    d = os.path.join(base, 'extracted-files')
    out = set()
    for sup, _tag, typ in read_tsv(os.path.join(d, 'substructures.tsv')):
        if typ.startswith(V71):
            out.add(short7(typ))
    return out


def generate(inputs_dir):
    v70 = gen_v7(os.path.join(inputs_dir, 'fs-gedcom-7.0'), '7.0')
    base71 = os.path.join(inputs_dir, 'fs-gedcom-7.1')
    v71 = gen_v7(base71, '7.1')
    v551 = gen_v551(os.path.join(inputs_dir, 'gedcom-go-5.5.1'))
    schemas = [('V551', Schema('5.5.1', '5.5.1', *v551[:4])),
               ('V70', Schema('7.0', '7.0', *v70[:4])),
               ('V71', Schema('7.1', '7.1', *v71[:4], v71_types=v71_types(base71)))]
    tags = sorted(set().union(*(s.tags() for _, s in schemas)))
    if len(tags) > 255:
        raise ValueError('more than 255 tags')
    tag_ids = {t: i for i, t in enumerate(tags)}
    sources = '\n'.join(f'//! * {k}: {n} @ {c} ({lic})' for k, (n, c, lic) in SOURCES.items())
    sources += '\n//! * 5.5.1 corrections: tools/spec-tables/551-errata.tsv (PDF page references)'
    hashes = []
    for files in (v551[4], v70[4], v71[4]):
        for f in files:
            rel = os.path.relpath(f, inputs_dir) if not f.startswith(os.path.dirname(ERRATA)) else 'tools/spec-tables/551-errata.tsv'
            hashes.append(f'//! * `{sha256(f)}` {rel}')
    tag_lines, line = [], '   '
    for t in tags:
        item = f' {rs_str(t)},'
        if len(line) + len(item) > 96:
            tag_lines.append(line)
            line = '   '
        line += item
    tag_lines.append(line)
    text = HEADER.format(sources=sources, hashes='\n'.join(hashes), ntags=len(tags),
                         tags='\n'.join(tag_lines))
    text += '\n' + '\n\n'.join(s.emit(name, tag_ids) for name, s in schemas) + '\n'
    return text


def crosscheck_551(registries, inputs_dir):
    """(superstructure tag, tag, cardinality) facts of GEDCOM-registries'
    5.5.1 structures against the generated 5.5.1 table: returns the
    differences as sorted lines. Nothing from the registries is copied."""
    import glob
    reg = {}
    tags = {}
    files = sorted(glob.glob(os.path.join(registries, 'structure', 'standard', '*-v551.yaml')))
    if not files:
        raise SystemExit(f'no *-v551.yaml under {registries}/structure/standard: run tools/fetch-corpora.sh --registries')
    for f in files:
        uri, tag, subs, section = None, None, {}, None
        with open(f, encoding='utf-8') as fh:
            for line in fh:
                m = re.match(r'^uri:\s*(\S+)', line)
                if m:
                    uri = m[1]
                m = re.match(r"^standard tag:\s*'?([A-Z0-9_]+)'?", line)
                if m:
                    tag = m[1]
                if re.match(r'^[a-z]', line):
                    section = line.split(':')[0]
                m = re.match(r'^\s+"([^"]+)":\s*"(\{[^}]+\})"', line)
                if m and section == 'substructures':
                    subs[m[1]] = m[2]
        tags[uri] = tag
        reg[uri] = subs
    theirs = set()
    for uri, subs in reg.items():
        for sub, c in subs.items():
            if tags.get(uri) and tags.get(sub):
                theirs.add((tags[uri], tags[sub], c.strip('{}')))
    subs, _, _, _, _ = gen_v551(os.path.join(inputs_dir, 'gedcom-go-5.5.1'))
    def sup_tag(ty):
        return ty.split('.')[-1].split('#')[0]
    ours = set()
    for sup, tag, ty, mn, mx in subs:
        if sup:
            ours.add((sup_tag(sup), tag, f'{mn}:{"M" if mx == 0 else mx}'))
    pairs_t = {(a, b) for a, b, _ in theirs}
    pairs_o = {(a, b) for a, b, _ in ours}
    out = [f'only in registries: {a}.{b}' for a, b in sorted(pairs_t - pairs_o)]
    out += [f'only in the table: {a}.{b}' for a, b in sorted(pairs_o - pairs_t)]
    card_t = {}
    for a, b, c in theirs:
        card_t.setdefault((a, b), set()).add(c)
    card_o = {}
    for a, b, c in ours:
        card_o.setdefault((a, b), set()).add(c)
    for k in sorted(pairs_t & pairs_o):
        if card_t[k] != card_o[k]:
            out.append(f'cardinality {k[0]}.{k[1]}: registries {sorted(card_t[k])}, table {sorted(card_o[k])}')
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    # The layout of tools/fetch-corpora.sh: $CORPORA_DIR/spec, else target/corpora/spec.
    corpora = os.environ.get('CORPORA_DIR') or os.path.join(ROOT, 'target', 'corpora')
    ap.add_argument('--inputs', default=os.path.join(corpora, 'spec'))
    ap.add_argument('--out', default=DEFAULT_OUT)
    ap.add_argument('--check', action='store_true')
    ap.add_argument('--crosscheck-551', metavar='REGISTRIES', help='compare the 5.5.1 table with a GEDCOM-registries checkout')
    a = ap.parse_args()
    if a.crosscheck_551:
        for line in crosscheck_551(a.crosscheck_551, a.inputs):
            print(line)
        return 0
    outputs = [(a.out, generate(a.inputs)), (ANSEL_OUT, generate_ansel())]
    stale = 0
    for path, text in outputs:
        if a.check:
            with open(path, encoding='utf-8') as f:
                if f.read() != text:
                    print(f'{path} is out of date: run tools/spec-tables/gen.py', file=sys.stderr)
                    stale = 1
            continue
        with open(path, 'w', encoding='utf-8', newline='\n') as f:
            f.write(text)
    return stale


if __name__ == '__main__':
    sys.exit(main())
