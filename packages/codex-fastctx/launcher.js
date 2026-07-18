#!/usr/bin/env node
'use strict';

process.env.FASTCTX_NPM_PACKAGE = 'codex-fastctx';
process.env.FASTCTX_NPM_LAUNCHER = __filename;
require('fastctx/launcher.js');
