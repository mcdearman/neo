#!/bin/sh
# Packs the Neo Meadow theme as a .vsix that VS Code can install:
#     dist/vscode/package.sh [output folder]
# A .vsix is a zip of the extension under extension/, with two small
# files beside it that say what is inside.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
src="$here/neo-meadow"
out=$(mkdir -p "${1:-$here/../../target}" && cd "${1:-$here/../../target}" && pwd)
version=$(sed -n 's/.*"version": *"\([^"]*\)".*/\1/p' "$src/package.json" | head -1)
name=$(sed -n 's/.*"name": *"\([^"]*\)".*/\1/p' "$src/package.json" | head -1)
publisher=$(sed -n 's/.*"publisher": *"\([^"]*\)".*/\1/p' "$src/package.json" | head -1)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/extension"
cp -R "$src/package.json" "$src/README.md" "$src/themes" "$work/extension/"
cat > "$work/[Content_Types].xml" <<XML
<?xml version="1.0" encoding="utf-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension=".json" ContentType="application/json"/><Default Extension=".md" ContentType="text/markdown"/><Default Extension=".vsixmanifest" ContentType="text/xml"/></Types>
XML
cat > "$work/extension.vsixmanifest" <<XML
<?xml version="1.0" encoding="utf-8"?>
<PackageManifest Version="2.0.0" xmlns="http://schemas.microsoft.com/developer/vsx-schema/2011">
  <Metadata>
    <Identity Language="en-US" Id="$name" Version="$version" Publisher="$publisher"/>
    <DisplayName>Neo Meadow</DisplayName>
    <Description xml:space="preserve">The Meadow colour scheme from NeoCode: a colour for each kind of name, after the Meadow REPL.</Description>
    <Tags>theme,color-theme,dark,light,meadow,neo</Tags>
    <Categories>Themes</Categories>
    <GalleryFlags>Public</GalleryFlags>
    <Properties>
      <Property Id="Microsoft.VisualStudio.Code.Engine" Value="^1.75.0"/>
      <Property Id="Microsoft.VisualStudio.Code.ExtensionKind" Value="ui,workspace"/>
    </Properties>
  </Metadata>
  <Installation><InstallationTarget Id="Microsoft.VisualStudio.Code"/></Installation>
  <Dependencies/>
  <Assets>
    <Asset Type="Microsoft.VisualStudio.Code.Manifest" Path="extension/package.json" Addressable="true"/>
    <Asset Type="Microsoft.VisualStudio.Services.Content.Details" Path="extension/README.md" Addressable="true"/>
  </Assets>
</PackageManifest>
XML
vsix="$out/$name-$version.vsix"
rm -f "$vsix"
(cd "$work" && zip -q -r "$vsix" "[Content_Types].xml" extension.vsixmanifest extension)
echo "$vsix"
