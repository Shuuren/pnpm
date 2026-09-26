import { TABLE_OPTIONS } from '@pnpm/cli.utils'
import { pickRegistryForPackage } from '@pnpm/config.pick-registry-for-package'
import { WANTED_LOCKFILE } from '@pnpm/constants'
import { lockfileToAuditRequest } from '@pnpm/deps.compliance.audit'
import { type SignatureIssue, type SignaturePackage, type SignatureVerificationResult, verifySignatures } from '@pnpm/deps.security.signatures'
import { PnpmError } from '@pnpm/error'
import { createGetAuthHeaderByURI } from '@pnpm/network.auth-header'
import { sanitizeInline } from '@pnpm/text.sanitize'
import { table } from '@zkochan/table'
import chalk from 'chalk'

import type { AuditOptions } from './audit.js'
import { createAuditNetworkOptions, loadAuditContext } from './auditContext.js'

export async function auditSignatures (opts: AuditOptions): Promise<{ exitCode: number, output: string }> {
  const { envLockfile, include, lockfile } = await loadAuditContext(opts)
  const auditRequest = lockfileToAuditRequest(lockfile, {
    envLockfile,
    include,
    resolvePeersFromWorkspaceRoot: opts.resolvePeersFromWorkspaceRoot,
  })
  const packages: SignaturePackage[] = Object.entries(auditRequest.request).flatMap(([name, versions]) => (
    versions.map((version) => ({ name, registry: pickRegistryForPackage(opts.registriesByScope, name), version }))
  ))
  const unresolvable = auditRequest.unresolvable.map((dep) => unresolvableSignatureIssue(dep, opts))
  if (packages.length === 0 && unresolvable.length === 0) {
    throw new PnpmError('AUDIT_NO_PACKAGES', 'No installed packages found to audit')
  }

  let result: SignatureVerificationResult = { audited: 0, invalid: [], missing: [], verified: 0 }
  if (packages.length > 0) {
    const getAuthHeader = createGetAuthHeaderByURI(opts.configByUri)
    const networkOptions = createAuditNetworkOptions(opts)
    result = await verifySignatures(packages, getAuthHeader, {
      ca: networkOptions.ca,
      cert: networkOptions.cert,
      configByUri: networkOptions.configByUri,
      httpProxy: networkOptions.httpProxy,
      httpsProxy: networkOptions.httpsProxy,
      key: networkOptions.key,
      localAddress: networkOptions.localAddress,
      maxSockets: networkOptions.maxSockets,
      networkConcurrency: opts.networkConcurrency,
      noProxy: networkOptions.noProxy,
      retry: networkOptions.retry,
      strictSsl: networkOptions.strictSsl,
      timeout: networkOptions.fetchTimeout,
    })
  }
  for (const issue of unresolvable) {
    result.invalid.push(issue)
    result.audited++
  }
  result.invalid.sort(compareSignatureIssue)

  return {
    exitCode: result.invalid.length > 0 || result.missing.length > 0 ? 1 : 0,
    output: opts.json ? JSON.stringify(result, null, 2) : renderSignatureVerificationResult(result),
  }
}

function unresolvableSignatureIssue (
  dep: { depPath: string, name: string, version: string },
  opts: AuditOptions
): SignatureIssue {
  const depPath = sanitizeInline(dep.depPath)
  return {
    name: sanitizeInline(dep.name),
    registry: pickRegistryForPackage(opts.registriesByScope, dep.name),
    version: sanitizeInline(dep.version),
    reason: `Broken lockfile: no entry for '${depPath}' in ${WANTED_LOCKFILE}`,
  }
}

function compareSignatureIssue (left: SignatureIssue, right: SignatureIssue): number {
  return `${left.name}@${left.version}`.localeCompare(`${right.name}@${right.version}`)
}

function renderSignatureVerificationResult (result: SignatureVerificationResult): string {
  const lines: string[] = []
  lines.push(`audited ${result.audited} package${result.audited === 1 ? '' : 's'}`)
  lines.push('')

  if (result.verified > 0) {
    lines.push(`${result.verified} package${result.verified === 1 ? ' has a' : 's have'} ${chalk.bold('verified')} registry signature${result.verified === 1 ? '' : 's'}`)
    lines.push('')
  }

  if (result.missing.length > 0) {
    lines.push(`${result.missing.length} package${result.missing.length === 1 ? ' is' : 's are'} ${chalk.redBright('missing')} registry signature${result.missing.length === 1 ? '' : 's'} but the registry is providing signing keys:`)
    lines.push('')
    lines.push(table(result.missing.map(({ name, registry, version }) => [chalk.red(`${name}@${version}`), registry]), TABLE_OPTIONS))
    lines.push('')
  }

  if (result.invalid.length > 0) {
    lines.push(`${result.invalid.length} package${result.invalid.length === 1 ? ' has an' : 's have'} ${chalk.redBright('invalid')} registry signature${result.invalid.length === 1 ? '' : 's'}:`)
    lines.push('')
    lines.push(table(result.invalid.map(({ name, reason, registry, version }) => [chalk.red(`${name}@${version}`), registry, reason ?? 'Invalid registry signature']), TABLE_OPTIONS))
    lines.push('')
    lines.push(result.invalid.length === 1
      ? 'Someone might have tampered with this package since it was published on the registry!'
      : 'Someone might have tampered with these packages since they were published on the registry!')
    lines.push('')
  }

  if (result.audited === 0 && result.invalid.length === 0 && result.missing.length === 0 && result.verified === 0) {
    lines.push('No dependencies were installed from a registry with signing keys')
    lines.push('')
  }

  return lines.join('\n')
}
